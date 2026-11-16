use super::*;

fn browse_host_operations() -> Vec<crate::adapter::AdapterHostOperation> {
    ["cwr.list_actions", crate::action_contract::INVOKE_ACTION_HOST_OPERATION]
        .into_iter()
        .map(|name| crate::adapter::AdapterHostOperation {
            name: name.into(),
            description: name.into(),
            input_schema: if name == crate::action_contract::INVOKE_ACTION_HOST_OPERATION {
                crate::action_contract::invoke_action_input_schema()
            } else {
                json!({"type":"object"})
            },
            output_schema: json!({"type":"object"}),
        })
        .collect()
}

fn selective_working_turn(
    store: &mut ConversationAdmissionStore,
    conversation: Option<&str>,
    event: &str,
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
        invocation.adapter_params["selective_history"] = json!(true);
        invocation.host_operations = browse_host_operations();
    }
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared).unwrap() else {
        panic!("start")
    };
    *turn
}

fn configured_working_turn(
    store: &mut ConversationAdmissionStore,
    conversation: Option<&str>,
    event: &str,
    selective_history: Option<bool>,
) -> AdmittedConversationTurn {
    configured_working_turn_with_tools(store, conversation, event, selective_history, true)
}

fn configured_working_turn_with_tools(
    store: &mut ConversationAdmissionStore,
    conversation: Option<&str>,
    event: &str,
    selective_history: Option<bool>,
    browse_tools: bool,
) -> AdmittedConversationTurn {
    let mut graph = fixture_graph_with_reviewer();
    let binding_ref = crate::v2026_09::test_fixture::binding_ref("helper");
    let binding = graph
        .actor_integrations
        .iter_mut()
        .find(|binding| binding.actor_integration_ref == binding_ref)
        .unwrap();
    if let Some(enabled) = selective_history {
        binding.adapter_params["selective_history"] = json!(enabled);
    }
    let mut prepared = store
        .prepare(
            &graph,
            &ingress(
                event,
                crate::v2026_09::test_fixture::actor_ref("person"),
                conversation,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                event,
            ),
            Some(binding_ref),
        )
        .unwrap();
    if browse_tools {
        if let ConversationPreparationOutcome::New(turn) = &mut prepared {
            turn.invocation.as_mut().unwrap().host_operations = browse_host_operations();
        }
    }
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared).unwrap() else {
        panic!("start")
    };
    *turn
}

#[test]
fn resumed_session_without_browse_tools_receives_full_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("legacy-tools.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = configured_working_turn_with_tools(&mut store, None, "first", Some(false), false);
    let claimed = claim_working(&mut store, &first);
    assert_ne!(claimed.adapter_invocation.thread["history_delivery"]["mode"], "selective");
    settle_working(&mut store, &claimed);
    for index in 0..7 {
        let event = format!("older-{index}");
        let prepared = store
            .prepare(
                &fixture_graph_with_reviewer(),
                &ingress(
                    &event,
                    crate::v2026_09::test_fixture::actor_ref("person"),
                    Some(&first.record.conversation_ref),
                    ConversationIngressAddressing::Unaddressed,
                    &event,
                ),
                None,
            )
            .unwrap();
        store.commit(prepared).unwrap();
    }
    let next = configured_working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "followup",
        None,
    );
    let followup = claim_working(&mut store, &next);
    assert_ne!(followup.adapter_invocation.thread["history_delivery"]["mode"], "selective");
    let messages = followup.adapter_invocation.thread["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| message["text"] == "older-0"));
    assert!(messages.iter().any(|message| message["text"] == "followup"));
}

#[test]
fn resumed_legacy_action_helper_cannot_select_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("legacy-action-schema.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = configured_working_turn_with_tools(&mut store, None, "first", Some(false), false);
    let mut claimed = claim_working(&mut store, &first);
    let mut operations = browse_host_operations();
    let action = operations
        .iter_mut()
        .find(|operation| operation.name == crate::action_contract::INVOKE_ACTION_HOST_OPERATION)
        .unwrap();
    action.input_schema = json!({
        "type":"object", "required":["activation","input"],
        "properties":{"activation":{"const":"@old--draft--brief"},"input":{}},
        "x-cwr-model-name":"cwr.invoke_action",
        "x-cwr-decoder-version":"cwr/1",
        "x-cwr-action-targets":{"@old--draft--brief":{"binding_ref":"old-binding"}}
    });
    claimed.adapter_invocation.host_operations = operations;
    store
        .retain_execution_invocation(
            &claimed.adapter_invocation.invocation_ref,
            &claimed.adapter_invocation,
        )
        .unwrap();
    settle_working(&mut store, &claimed);
    let next = configured_working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "followup",
        None,
    );
    let followup = claim_working(&mut store, &next);
    assert!(followup.adapter_invocation.thread["working_conversation"]["state"].is_object());
    assert_ne!(followup.adapter_invocation.thread["history_delivery"]["mode"], "selective");
    assert_eq!(
        followup.adapter_invocation.host_operations[1].input_schema["x-cwr-decoder-version"],
        "cwr/1"
    );
}

#[test]
fn selective_snapshot_projects_visualization_before_testing_message_size() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("visualization-selective.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let prepared = store
        .prepare(
            &fixture_graph_with_reviewer(),
            &ingress(
                "chart",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Unaddressed,
                "chart",
            ),
            None,
        )
        .unwrap();
    let ConversationAdmissionOutcome::Started(chart) = store.commit(prepared).unwrap() else {
        panic!("start chart conversation")
    };
    let marker = format!(
        "\u{e200}visualize\u{e202}{}\u{e201}",
        json!({"path": format!("/private/{}.html", "x".repeat(5_000)), "title":"Saved chart"})
    );
    store
        .transact(|state| {
            let message = &mut state.conversations[0].messages[0];
            message.text = marker.clone();
            message.visualizations = vec![ConversationVisualization {
                marker_index: 0,
                title: Some("Saved chart".into()),
                mode: None,
                artifact_ref: None,
                error: None,
            }];
            Ok(())
        })
        .unwrap();
    let next = selective_working_turn(&mut store, Some(&chart.conversation.conversation_ref), "read chart");
    let claimed = claim_working(&mut store, &next);
    let messages = claimed.adapter_invocation.thread["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| message["text"] == "Saved chart"));
    assert!(!serde_json::to_string(messages).unwrap().contains("/private/"));
    store
        .retain_execution_invocation(
            &claimed.adapter_invocation.invocation_ref,
            &claimed.adapter_invocation,
        )
        .unwrap();
    let retained = retained_context_snapshot(&store, &claimed.adapter_invocation.invocation_ref);
    let restored = context::restore(&store.state, &retained["thread"]).unwrap();
    assert_eq!(restored["messages"], claimed.adapter_invocation.thread["messages"]);
}

#[test]
fn default_on_can_switch_to_full_history_and_back_without_losing_omitted_messages() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("history-toggle.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = configured_working_turn(&mut store, None, "first", None);
    let claimed = claim_working(&mut store, &first);
    assert_eq!(claimed.adapter_invocation.thread["history_delivery"]["mode"], "selective");
    settle_working(&mut store, &claimed);

    let large = format!("omitted log: {}", "x".repeat(20_000));
    let prepared = store
        .prepare(
            &fixture_graph_with_reviewer(),
            &ingress(
                "large-log",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                &large,
            ),
            None,
        )
        .unwrap();
    store.commit(prepared).unwrap();

    let second = configured_working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "second",
        Some(false),
    );
    let full = claim_working(&mut store, &second);
    assert!(full.adapter_invocation.thread["working_conversation"]["state"].is_object());
    assert_ne!(full.adapter_invocation.thread["history_delivery"]["mode"], "selective");
    let messages = full.adapter_invocation.thread["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| message["text"] == "first"));
    assert!(messages.iter().any(|message| message["text"] == large));
    assert!(messages.iter().any(|message| message["text"] == "second"));
    settle_working(&mut store, &full);

    let third = configured_working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "third",
        Some(true),
    );
    let selected = claim_working(&mut store, &third);
    assert!(selected.adapter_invocation.thread["working_conversation"]["state"].is_object());
    assert_eq!(selected.adapter_invocation.thread["history_delivery"]["mode"], "selective");
    assert!(selected.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"] == "third"));
}

#[test]
fn isolated_codex_binding_keeps_full_history_when_flag_is_omitted() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("isolated-history.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let mut graph = fixture_graph_with_reviewer();
    let binding_ref = crate::v2026_09::test_fixture::binding_ref("helper");
    graph
        .actor_integrations
        .iter_mut()
        .find(|binding| binding.actor_integration_ref == binding_ref)
        .unwrap()
        .adapter_params["isolated"] = json!(true);
    let prepared = store
        .prepare(
            &graph,
            &ingress(
                "isolated",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                "isolated",
            ),
            Some(binding_ref),
        )
        .unwrap();
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared).unwrap() else {
        panic!("start")
    };
    let claimed = claim_working(&mut store, &turn);
    assert_ne!(claimed.adapter_invocation.thread["history_delivery"]["mode"], "selective");
    assert_eq!(claimed.adapter_invocation.thread["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn queued_binding_with_revoked_cwr_action_permission_keeps_full_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("no-browse-permission.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let graph = fixture_graph_with_reviewer();
    let binding_ref = crate::v2026_09::test_fixture::binding_ref("helper");
    let first = store
        .prepare(
            &graph,
            &ingress(
                "old",
                crate::v2026_09::test_fixture::actor_ref("person"),
                None,
                ConversationIngressAddressing::Unaddressed,
                "old",
            ),
            None,
        )
        .unwrap();
    let ConversationAdmissionOutcome::Started(first) = store.commit(first).unwrap() else {
        panic!("start")
    };
    let prepared = store
        .prepare(
            &graph,
            &ingress(
                "new",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.conversation.conversation_ref),
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                "new",
            ),
            Some(binding_ref),
        )
        .unwrap();
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared).unwrap() else {
        panic!("start")
    };
    let mut current_graph = graph;
    current_graph
        .actor_integrations
        .iter_mut()
        .find(|binding| binding.actor_integration_ref == binding_ref)
        .unwrap()
        .actor_integration_params["cwr_action_actor_refs"] = json!([]);
    store.response_graph = Some(current_graph);
    let claimed = claim_working(&mut store, &turn);
    assert_ne!(claimed.adapter_invocation.thread["history_delivery"]["mode"], "selective");
    assert_eq!(claimed.adapter_invocation.thread["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn selective_history_skips_huge_new_progress_and_keeps_private_state_on_retry() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("selective.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = selective_working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    let first_thread = &claimed.adapter_invocation.thread;
    assert_eq!(first_thread["history_delivery"]["mode"], "selective");
    settle_working(&mut store, &claimed);
    let large = format!("unrelated log: {}", "x".repeat(2_000_000));
    let prepared = store
        .prepare(
            &fixture_graph_with_reviewer(),
            &ingress(
                "large-log",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Unaddressed,
                &large,
            ),
            None,
        )
        .unwrap();
    store.commit(prepared).unwrap();
    let second =
        selective_working_turn(&mut store, Some(&first.record.conversation_ref), "followup");
    let next = claim_working(&mut store, &second);
    let thread = &next.adapter_invocation.thread;
    assert!(thread["working_conversation"]["state"].is_object());
    assert_eq!(thread["history_delivery"]["mode"], "selective");
    assert_eq!(
        thread["messages"].as_array().unwrap().last().unwrap()["text"],
        "followup"
    );
    assert!(serde_json::to_vec(&thread["messages"]).unwrap().len() < 16_000);
    assert!(thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Large body omitted")));
    store
        .retain_execution_invocation(
            &next.adapter_invocation.invocation_ref,
            &next.adapter_invocation,
        )
        .unwrap();
    let retained = retained_context_snapshot(&store, &next.adapter_invocation.invocation_ref);
    let restored = context::restore(&store.state, &retained["thread"]).unwrap();
    assert_eq!(restored["messages"], thread["messages"]);
}

#[path = "selective/progress.rs"]
mod progress;

#[test]
fn selective_history_keeps_a_queued_request_omitted_by_the_prior_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("queued-selective.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = selective_working_turn(&mut store, None, "first");
    let second = selective_working_turn(&mut store, Some(&first.record.conversation_ref), "second");
    for index in 0..5 {
        let event = format!("unrelated-{index}");
        let prepared = store
            .prepare(
                &fixture_graph_with_reviewer(),
                &ingress(
                    &event,
                    crate::v2026_09::test_fixture::actor_ref("person"),
                    Some(&first.record.conversation_ref),
                    ConversationIngressAddressing::Unaddressed,
                    &event,
                ),
                None,
            )
            .unwrap();
        store.commit(prepared).unwrap();
    }
    let claimed = claim_working(&mut store, &first);
    assert!(!claimed.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"] == "second"));
    let prior_indices = claimed.adapter_invocation.thread["history_delivery"]["selected_message_indices"].as_array().unwrap().clone();
    settle_working(&mut store, &claimed);
    let next = claim_working(&mut store, &second);
    assert!(next.adapter_invocation.thread["history_delivery"]["selected_message_indices"]
        .as_array().unwrap().iter().all(|index| !prior_indices.contains(index)));
    assert!(next.adapter_invocation.thread["working_conversation"]["state"].is_object());
    assert!(next.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"] == "second"));
    assert!(!next.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"] == "unrelated-0"));
    assert!(
        next.adapter_invocation.thread["messages"]
            .as_array()
            .unwrap()
            .len()
            <= 4
    );
}

#[test]
fn selective_history_does_not_truncate_a_named_action() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("named-action-selective.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    settle_working(&mut store, &claimed);
    for index in 0..7 {
        let event = format!("earlier-{index}");
        let prepared = store
            .prepare(
                &fixture_graph_with_action(),
                &ingress(
                    &event,
                    crate::v2026_09::test_fixture::actor_ref("person"),
                    Some(&first.record.conversation_ref),
                    ConversationIngressAddressing::Unaddressed,
                    &event,
                ),
                None,
            )
            .unwrap();
        store.commit(prepared).unwrap();
    }
    let mut prepared = store
        .prepare(
            &fixture_graph_with_action(),
            &ingress(
                "named-action",
                crate::v2026_09::test_fixture::actor_ref("person"),
                Some(&first.record.conversation_ref),
                ConversationIngressAddressing::Action {
                    actor_ref: crate::v2026_09::test_fixture::actor_ref("helper").into(),
                    activation: "@helper--draft--brief".into(),
                    action_name: "draft--brief".into(),
                    input: json!({}),
                },
                "@helper--draft--brief",
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap();
    let ConversationPreparationOutcome::New(turn) = &mut prepared else {
        panic!("new named action")
    };
    let invocation = turn.invocation.as_mut().unwrap();
    assert!(!invocation.conversation_continuity);
    invocation.adapter_params["selective_history"] = json!(true);
    let ConversationAdmissionOutcome::Started(action) = store.commit(prepared).unwrap() else {
        panic!("started named action")
    };
    let claimed = claim_working(&mut store, &action);
    let thread = &claimed.adapter_invocation.thread;
    assert_ne!(thread["history_delivery"]["mode"], "selective");
    assert_eq!(thread["messages"].as_array().unwrap().len(), 10);
    assert!(thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"] == "earlier-0"));
}

#[path = "selective/replay.rs"]
mod replay;

#[path = "selective_stopped.rs"]
mod stopped;
