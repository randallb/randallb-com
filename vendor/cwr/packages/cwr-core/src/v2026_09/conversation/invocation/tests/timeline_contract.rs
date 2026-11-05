fn start_contract_child(
    store: &mut ConversationAdmissionStore,
    parent_ref: &str,
    child_ref: &str,
    call_id: &str,
) -> ConversationMessage {
    let mut request = child_request(child_ref, call_id);
    request.parent_invocation_ref = Some(parent_ref.into());
    request.origin = InvocationOrigin::HostCall {
        caller_invocation_ref: parent_ref.into(),
        call_id: call_id.into(),
    };
    let caller = store
        .invocation_record(parent_ref)
        .unwrap()
        .target_actor_ref
        .clone();
    request.requester_actor_ref = caller.clone();
    let call = crate::adapter::AdapterCall {
        invocation_ref: parent_ref.into(),
        call_id: call_id.into(),
        operation: "cwr.invoke_action".into(),
        input: json!({"activation":"@child--draft--brief"}),
    };
    store
        .start_child_from_host_action_call(
            &call,
            ConversationHostActionCall {
                conversation_ref: CONVERSATION_REF,
                caller_invocation_ref: parent_ref,
                host_call_id: call_id,
                nested_invocation_ref: child_ref,
                caller_actor_ref: &caller,
                target_actor_ref: "cwp://com.example.fixture/actors/child",
                activation: "@child--draft--brief",
                input: json!({"topic":"launch"}),
                text: "Draft brief".into(),
            },
            request,
            child_adapter_invocation(child_ref),
        )
        .unwrap()
        .0
}

fn timeline_page(
    store: &ConversationAdmissionStore,
    window: ConversationRecordWindow<'_>,
) -> ConversationTimelinePage {
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    operation
        .conversation_timeline(CONVERSATION_REF, window)
        .unwrap()
}

fn capture_timeline_contract(store: &ConversationAdmissionStore, name: &str) -> serde_json::Value {
    let snapshot = crate::v2026_09::host::project_conversation_with_authoritative_work(
        store.conversation(CONVERSATION_REF).unwrap(),
        store,
    );
    let page = timeline_page(
        store,
        ConversationRecordWindow {
            first: Some(500),
            ..Default::default()
        },
    );
    let public_work = snapshot
        .activity
        .iter()
        .filter(|activity| {
            matches!(
                activity.event,
                ConversationActivityEvent::WorkLifecycle { .. }
                    | ConversationActivityEvent::WorkUpdate { .. }
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let indexed_work = page
        .activity
        .iter()
        .filter(|activity| {
            matches!(
                activity.event,
                ConversationActivityEvent::WorkLifecycle { .. }
                    | ConversationActivityEvent::WorkUpdate { .. }
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        indexed_work, public_work,
        "{name}: snapshot/indexed authoritative work"
    );
    let mut singles = Vec::new();
    for position in 0..page.anchors.len() {
        let after = position
            .checked_sub(1)
            .map(|position| ConversationPositionKey {
                position: i64::try_from(position).unwrap(),
                revision: Some(page.revision.clone()),
            });
        let single = timeline_page(
            store,
            ConversationRecordWindow {
                after: after.as_ref(),
                first: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(single.anchors, [page.anchors[position].clone()]);
        singles.push(single);
    }
    serde_json::json!({"name": name, "snapshot": snapshot, "page": page, "singles": singles})
}

fn publication_write_audit(store:&ConversationAdmissionStore,action:cwpdb_store::conversation::PublicationAudit)->cwpdb_store::conversation::PublicationWriteCounts {
    store.backend.lock().unwrap().storage.audit_publication_writes(action).unwrap()
}
fn audit_timeline_writes(store: &ConversationAdmissionStore) {
    publication_write_audit(store,cwpdb_store::conversation::PublicationAudit::Start);
}
fn assert_activity_only_writes(store: &ConversationAdmissionStore, count: i64) {
    let writes=publication_write_audit(store,cwpdb_store::conversation::PublicationAudit::Drain);
    assert_eq!(writes.message + writes.anchor,0,"activity updates must not rewrite message/anchor rows");
    assert_eq!(writes.activity,count as u64);
}

#[test]
fn timeline_projection_contract_matches_snapshot_through_real_mutations() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("timeline.sqlite");
    let mut store = open(&path);
    store.commit(prepared_root()).unwrap();
    let mut cases = vec![capture_timeline_contract(&store, "admitted")];
    let admitted = timeline_page(
        &store,
        ConversationRecordWindow {
            first: Some(1),
            ..Default::default()
        },
    );
    audit_timeline_writes(&store);
    let execution = store
        .execution_conversation(ROOT_INVOCATION_REF)
        .unwrap()
        .clone();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    assert_eq!(
        store.execution_conversation(ROOT_INVOCATION_REF),
        Some(&execution),
        "claim must exercise a ledger-only update"
    );
    assert_activity_only_writes(&store, 1);
    cases.push(capture_timeline_contract(
        &store,
        "claimed_without_progress",
    ));
    assert_eq!(cases[0]["page"]["revision"], cases[1]["page"]["revision"]);
    let cursor = ConversationPositionKey {
        position: 0,
        revision: Some(admitted.revision),
    };
    assert_eq!(
        timeline_page(
            &store,
            ConversationRecordWindow {
                after: Some(&cursor),
                first: Some(1),
                ..Default::default()
            }
        )
        .anchors
        .len(),
        1
    );

    store
        .record_invocation_progress_with_activity(ROOT_INVOCATION_REF, 102, "Parent progress")
        .unwrap();
    assert_activity_only_writes(&store, 1);
    cases.push(capture_timeline_contract(&store, "progress"));
    store
        .rename_conversation(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/requester",
            "Renamed",
        )
        .unwrap();
    assert_activity_only_writes(&store, 0);
    assert_eq!(
        timeline_page(
            &store,
            ConversationRecordWindow {
                after: Some(&cursor),
                first: Some(1),
                ..Default::default()
            }
        )
        .anchors
        .len(),
        1,
        "progress and summary changes preserve the anchor cursor"
    );
    let child_ref = "cwp://com.example.runtime/invocations/contract-child";
    start_contract_child(&mut store, ROOT_INVOCATION_REF, child_ref, "contract-child");
    store.claim_invocation(child_ref, 21).unwrap();
    store
        .record_invocation_progress_with_activity(child_ref, 102, "Nested progress")
        .unwrap();
    cases.push(capture_timeline_contract(&store, "nested_progress"));
    assert!(cases.last().unwrap()["page"]["activity"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["event"]["awaited_by_invocation_ref"] == ROOT_INVOCATION_REF));
    let grandchild = "cwp://com.example.runtime/invocations/contract-grandchild";
    let grandchild_call =
        start_contract_child(&mut store, child_ref, grandchild, "contract-grandchild");
    store.claim_invocation(grandchild, 23).unwrap();
    store
        .record_invocation_progress_with_activity(grandchild, 102, "Grandchild progress")
        .unwrap();
    cases.push(capture_timeline_contract(&store, "grandchild_progress"));
    store
        .publish_child_action_terminal(
            grandchild,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Grandchild done".into(),
                result: Some(json!({"done":true})),
                error: None,
            },
            "Grandchild done".into(),
            29,
        )
        .unwrap();
    store
        .publish_child_action_terminal(
            child_ref,
            InvocationTerminal {
                status: ActionResponseStatus::COMPLETED,
                summary: "Brief ready".into(),
                result: Some(json!({"brief":"Ready"})),
                error: None,
            },
            "Brief ready".into(),
            30,
        )
        .unwrap();
    store
        .complete(ROOT_INVOCATION_REF, json!({"text":"Done"}))
        .unwrap();
    cases.push(capture_timeline_contract(&store, "completed"));
    let before_reply = timeline_page(
        &store,
        ConversationRecordWindow {
            first: Some(1),
            ..Default::default()
        },
    );

    // An unrelated action reply exposes replyTo from a one-anchor page without
    // hydrating the work that happened to be started by the replied-to message.
    store
        .append_action_result(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/requester",
            "@worker--draft--brief",
            json!({}),
            ConversationActionOutcome::Succeeded,
            "Follow-up reply".into(),
            &grandchild_call.message_ref,
        )
        .unwrap();
    cases.push(capture_timeline_contract(&store, "cross_page_reply"));
    let mut reader = store.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    assert!(operation
        .conversation_timeline(
            CONVERSATION_REF,
            ConversationRecordWindow {
                after: Some(&ConversationPositionKey {
                    position: 0,
                    revision: Some(before_reply.revision)
                }),
                first: Some(1),
                ..Default::default()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("earlier revision"));
    drop(operation);
    drop(reader);
    let reply = timeline_page(
        &store,
        ConversationRecordWindow {
            last: Some(1),
            ..Default::default()
        },
    );
    assert_eq!(reply.messages.len(), 4);
    assert_eq!(
        reply.activity.len(),
        6,
        "reply context needs three lifecycle records and three message timestamps, not their work"
    );
    let before_reopen = cases.last().unwrap().clone();
    drop(store);
    let reopened = open(&path);
    assert_eq!(
        capture_timeline_contract(&reopened, "cross_page_reply"),
        before_reopen
    );

    cases.extend(wait_timeline_contract());
    let mut contract = serde_json::json!(cases);
    normalize_timeline_contract(&mut contract, &mut std::collections::BTreeMap::new());
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/contracts/timeline-projection.json");
    if std::env::var_os("UPDATE_TIMELINE_CONTRACT").is_some() {
        fs::write(
            &fixture,
            format!("{}\n", serde_json::to_string_pretty(&contract).unwrap()),
        )
        .unwrap();
    }
    let expected: serde_json::Value = serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
    assert!(contract == expected, "CWR/desktop fixture changed; regenerate with UPDATE_TIMELINE_CONTRACT=1 and review the diff");
}

fn wait_timeline_contract() -> Vec<serde_json::Value> {
    let directory = tempfile::tempdir().unwrap();
    let mut store = open(&directory.path().join("wait.sqlite"));
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let child = "cwp://com.example.runtime/invocations/wait-child";
    start_contract_child(&mut store, ROOT_INVOCATION_REF, child, "wait-child");
    store.claim_invocation(child, 21).unwrap();
    store
        .record_invocation_progress_with_activity(child, 102, "Working while parent waits")
        .unwrap();
    // Import a valid suspension produced by the older persisted-await protocol.
    let mut ledger = serde_json::to_value(&store.state.invocation_ledger).unwrap();
    ledger["invocations"][ROOT_INVOCATION_REF]["state"] = json!("waiting");
    ledger["invocations"][ROOT_INVOCATION_REF]["attempt_active"] = json!(false);
    ledger["waits"] = json!([{
        "waiter_invocation_ref": ROOT_INVOCATION_REF, "awaited_invocation_ref": child,
        "call_id": "contract-await", "state": "pending"
    }]);
    let mut staged = store.state.clone();
    staged.invocation_ledger = serde_json::from_value(ledger).unwrap();
    store.replace_state(staged).unwrap();
    let pending = capture_timeline_contract(&store, "pending_wait");
    audit_timeline_writes(&store);
    store
        .transact(|state| state.invocation_ledger.settle_legacy_suspended(40))
        .unwrap();
    assert_activity_only_writes(&store, 3);
    let settled = capture_timeline_contract(&store, "settled_wait");
    assert_eq!(pending["page"]["revision"], settled["page"]["revision"]);
    vec![pending, settled]
}

#[test]
fn timeline_child_lifecycle_tracks_parent_host_relationship_changes() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = open(&directory.path().join("parent.sqlite"));
    store.commit(prepared_root()).unwrap();
    store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
    let child = "cwp://com.example.runtime/invocations/late-parent-call";
    let call_id = "late-parent-call";
    let source = store
        .append_host_action_call(ConversationHostActionCall {
            conversation_ref: CONVERSATION_REF,
            caller_invocation_ref: ROOT_INVOCATION_REF,
            host_call_id: call_id,
            nested_invocation_ref: child,
            caller_actor_ref: "cwp://com.example.fixture/actors/worker",
            target_actor_ref: "cwp://com.example.fixture/actors/child",
            activation: "@child--draft--brief",
            input: json!({}),
            text: "Draft brief".into(),
        })
        .unwrap();
    store
        .start_child_invocation(
            child_request(child, call_id),
            &source.message_ref,
            child_adapter_invocation(child),
        )
        .unwrap();
    let before = timeline_page(
        &store,
        ConversationRecordWindow {
            first: Some(500),
            ..Default::default()
        },
    );
    let awaited_by = |page: &ConversationTimelinePage| {
        page.activity.iter().find_map(|activity| {
            if let ConversationActivityEvent::WorkLifecycle {
                invocation_ref,
                awaited_by_invocation_ref,
                ..
            } = &activity.event
            {
                if invocation_ref == child {
                    return Some(awaited_by_invocation_ref.clone());
                }
            }
            None
        })
    };
    assert_eq!(awaited_by(&before), Some(None));
    audit_timeline_writes(&store);
    store
        .append_execution_activity(
            ROOT_INVOCATION_REF,
            ConversationActivityEvent::HostCall {
                call_id: call_id.into(),
                operation: "cwr.invoke_action".into(),
                input: json!({}),
            },
        )
        .unwrap();
    assert_activity_only_writes(&store, 1);
    let after = timeline_page(
        &store,
        ConversationRecordWindow {
            first: Some(500),
            ..Default::default()
        },
    );
    assert_eq!(awaited_by(&after), Some(Some(ROOT_INVOCATION_REF.into())));
    assert_eq!(before.revision, after.revision);
    capture_timeline_contract(&store, "changed_parent_relationship");
}

// Normalize generated identities, wall time and interleaving between executions
// (timestamp/sequence ties otherwise sort by random UUID). Per-execution event
// order remains intact. Every capture first compares the actual ordered records.
fn normalize_timeline_contract(
    value: &mut serde_json::Value,
    refs: &mut std::collections::BTreeMap<String, String>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if key.ends_with("_unix_ms") && value.is_number() {
                    *value = json!(1);
                } else {
                    if let Some(activities) = value.as_array_mut().filter(|_| key == "activity") {
                        // Invocation IDs in this scenario are fixed. Sort before
                        // assigning generated IDs, including non-work events.
                        activities.sort_by_key(timeline_contract_activity_key);
                    }
                    normalize_timeline_contract(value, refs);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                normalize_timeline_contract(value, refs);
            }
        }
        serde_json::Value::String(reference) if reference.starts_with(RUNTIME_ROOT) => {
            let next = refs.len();
            *reference = refs
                .entry(reference.clone())
                .or_insert_with(|| format!("{RUNTIME_ROOT}/contract/{next}"))
                .clone();
        }
        _ => {}
    }
}

fn timeline_contract_activity_key(activity: &serde_json::Value) -> (u64, String, String) {
    (
        activity["sequence"].as_u64().unwrap_or_default(),
        activity["event"]["invocation_ref"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        activity["event"]["kind"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}
