use super::*;

#[test]
fn selective_coverage_notice_reaches_the_provider_boundary() {
    let mut source = observation();
    source["history_delivery"] = json!({
        "mode":"selective","message_count":9,"selected_message_indices":[7,8],
        "coverage":"Earlier messages remain available through browsing"
    });
    let mut plan = HistoryPreparation::new(&config(source)).unwrap();
    let boundary = read_history_unit(&mut plan.spool, &plan.units[1]).unwrap();
    let boundary: Value = serde_json::from_str(&boundary).unwrap();
    assert_eq!(boundary["metadata"]["history_delivery"]["selected_message_indices"], json!([7,8]));
    assert_eq!(boundary["metadata"]["history_delivery"]["mode"], "selective");
}

#[test]
fn source_boundary_excludes_runtime_bookkeeping_and_compacts_initial_discovery() {
    let mut source = observation();
    source["conversation_ref"] = json!("conversation-one");
    source["working_conversation"] = json!({"ref":"private-conversation",
        "history_gap":"Earlier private work was unavailable."});
    source["coworkers"] = json!({"kind":"coworkers","coworkers":[{
        "coworker_ref":"long-coworker-ref","runtime_ref":"long-runtime-ref",
        "role_refs":["long-role-ref"],"handle":"@helper","name":"Helper",
        "description":"Helps with focused work.","participant":false,"delivery":"configured",
        "actions":["@helper--do--work"],"bindings":[{"readiness":{"message":"ready"}}]
    }]});
    let mut plan = HistoryPreparation::new(&config(source)).unwrap();
    let boundary = read_history_unit(&mut plan.spool, &plan.units[1]).unwrap();
    let boundary: Value = serde_json::from_str(&boundary).unwrap();
    let metadata = &boundary["metadata"];
    assert_eq!(metadata["conversation_ref"], "conversation-one");
    assert_eq!(metadata["coworkers"]["coworkers"][0]["handle"], "@helper");
    assert_eq!(metadata["coworkers"]["coworkers"][0]["description"], "Helps with focused work.");
    assert!(metadata.get("working_conversation").is_none());
    assert!(metadata.get("context_snapshot").is_none());
    assert_eq!(metadata["history_gap"], "Earlier private work was unavailable.");
    assert!(metadata["coworkers"]["coworkers"][0].get("coworker_ref").is_none());
    assert!(metadata["coworkers"]["coworkers"][0].get("bindings").is_none());
    assert!(metadata["coworkers"]["coworkers"][0].get("actions").is_none());
}

#[test]
fn retained_source_only_reintroduces_discovery_when_it_changes() {
    let mut source = observation();
    source["coworkers"] = json!({"kind":"coworkers","coworkers":[{
        "handle":"@helper","name":"Helper","description":"Helps","participant":true,
        "bindings":[{"readiness":{"message":"ready"}}]
    }]});
    let digest = model_coworker_directory_digest(&source["coworkers"]).unwrap();
    source["working_conversation"] = json!({"ref":"working-one","state":{
        "coworker_directory_digest":digest
    }});
    let unchanged = model_source_metadata(source.as_object().unwrap()).unwrap();
    assert!(unchanged.get("coworkers").is_none());

    source["coworkers"]["coworkers"][0]["bindings"][0]["readiness"]["message"] =
        json!("still ready");
    let internal_change = model_source_metadata(source.as_object().unwrap()).unwrap();
    assert!(internal_change.get("coworkers").is_none());

    source["coworkers"]["coworkers"][0]["description"] = json!("Now helps differently");
    let changed = model_source_metadata(source.as_object().unwrap()).unwrap();
    assert_eq!(
        changed["coworkers"]["coworkers"][0]["description"],
        "Now helps differently"
    );
}

#[test]
fn retained_directory_digest_is_persisted_for_the_next_turn() {
    let mut source = observation();
    source["coworkers"] = json!({"kind":"coworkers","coworkers":[]});
    source["working_conversation"] = json!({"ref":"working-one"});
    let expected = model_coworker_directory_digest(&source["coworkers"]).unwrap();
    let mut plan = HistoryPreparation::new(&config(source)).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    let mut handler = Handler::default();
    plan.retain_working_state(&mut handler).unwrap();
    assert_eq!(
        handler.checkpoints.last().unwrap()["coworker_directory_digest"],
        expected
    );
}

fn with_settled_codex_state(mut source: Value, directory_digest: Option<String>) -> Value {
    let initialization_digest = HistoryPreparation::new(&config(source.clone()))
        .unwrap()
        .checkpoint
        .initialization_digest;
    let mut state = json!({
        "version":3,"provider":"codex","idle":true,
        "working_ref":"working-one","thread_id":"settled-thread",
        "initialization_digest":initialization_digest,
        "invocation_ref":"settled-invocation","context_snapshot":{},
        "estimator":cwr::token_estimation::ESTIMATOR,
        "last_total_tokens":100,"model_context_window":258400,
        "usage_turn_id":"settled-turn","usage_stale":false
    });
    if let Some(digest) = directory_digest {
        state["coworker_directory_digest"] = json!(digest);
    }
    source["working_conversation"] = json!({"ref":"working-one","state":state});
    source
}

fn assert_directory_projection_appends(source: Value, expect_directory: bool) {
    let mut config = config(source);
    let mut plan = HistoryPreparation::new(&config).unwrap();
    let boundary = plan
        .units
        .iter()
        .find(|unit| unit.source_id == "requesting/source-boundary")
        .unwrap();
    let boundary: Value = serde_json::from_str(
        &read_history_unit(&mut plan.spool, boundary).unwrap(),
    )
    .unwrap();
    assert_eq!(boundary["metadata"].get("coworkers").is_some(), expect_directory);

    let next = attributed_history_text(
        &plan.units[2],
        &read_history_unit(&mut plan.spool, &plan.units[2]).unwrap(),
    );
    let mut old_checkpoint = serde_json::to_value(&plan.checkpoint).unwrap();
    old_checkpoint["version"] = json!(4);
    old_checkpoint["thread_id"] = json!("old-preparation-thread");
    old_checkpoint["confirmed_items"] = json!(2);
    old_checkpoint
        .as_object_mut()
        .unwrap()
        .remove("source_delivery");
    config.continuation = Some(old_checkpoint);

    let mut recovered = HistoryPreparation::new(&config).unwrap();
    assert!(recovered.resume_thread().is_none());
    assert_eq!(recovered.provider_resume_thread(), Some("settled-thread"));
    assert!(recovered.delivers_history_with_turn());
    assert_eq!(recovered.checkpoint.version, PREPARATION_CHECKPOINT_VERSION);
    assert_eq!(recovered.checkpoint.confirmed_items, 0);
    let input = recovered
        .turn_input(&config.observation["action_request"])
        .unwrap();
    assert_eq!(input[2]["text"], next);
}

#[test]
fn old_interrupted_projection_rebuilds_as_an_atomic_append() {
    let directory = json!({"kind":"coworkers","coworkers":[{
        "handle":"@helper","name":"Helper","description":"Current help",
        "participant":true,"delivery":"configured"
    }]});
    let current_digest = model_coworker_directory_digest(&directory).unwrap();
    let mut unchanged = observation();
    unchanged["coworkers"] = directory.clone();
    assert_directory_projection_appends(
        with_settled_codex_state(unchanged, Some(current_digest)),
        false,
    );

    let prior_directory = json!({"kind":"coworkers","coworkers":[{
        "handle":"@helper","name":"Helper","description":"Old help",
        "participant":true,"delivery":"configured"
    }]});
    let mut changed = observation();
    changed["coworkers"] = directory;
    assert_directory_projection_appends(
        with_settled_codex_state(
            changed,
            Some(model_coworker_directory_digest(&prior_directory).unwrap()),
        ),
        true,
    );
}

#[test]
fn old_full_metadata_checkpoint_rebuilds_from_the_settled_provider_thread() {
    let mut source = observation();
    source["coworkers"] = json!({"kind":"coworkers","coworkers":[{
        "handle":"@helper","name":"Helper","description":"Helps",
        "participant":true,"delivery":"configured",
        "runtime_ref":"r".repeat(HISTORY_PART_BYTES * 3),
        "actions":["@helper--do--work"]
    }]});
    let source = with_settled_codex_state(source, None);

    // Version 3 serialized all source metadata. Confirm this fixture creates a
    // multi-part old boundary whose indexes do not match the concise projection.
    let object = source.as_object().unwrap();
    let old_metadata = object
        .iter()
        .filter(|(key, _)| {
            !matches!(key.as_str(), "messages" | "requesting_context" | "action_request")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let mut old_spool = tempfile::NamedTempFile::new().unwrap();
    let mut old_boundary_units = Vec::new();
    append_history_units(
        &mut old_spool,
        &mut old_boundary_units,
        "requesting/source-boundary".into(),
        &json!({"context_source":"requesting","metadata":old_metadata}),
        HISTORY_PART_BYTES,
    )
    .unwrap();
    assert!(old_boundary_units.len() >= 3);

    let mut config = config(source);
    let current = HistoryPreparation::new(&config).unwrap();
    assert_eq!(
        current
            .units
            .iter()
            .filter(|unit| unit.source_id == "requesting/source-boundary")
            .count(),
        1
    );
    let mut old_checkpoint = serde_json::to_value(&current.checkpoint).unwrap();
    old_checkpoint["version"] = json!(3);
    old_checkpoint["thread_id"] = json!("old-preparation-thread");
    old_checkpoint["confirmed_items"] = json!(2);
    config.continuation = Some(old_checkpoint);

    let rebuilt = HistoryPreparation::new(&config).unwrap();
    assert_eq!(rebuilt.prior_thread.as_deref(), Some("settled-thread"));
    assert_eq!(rebuilt.provider_resume_thread(), Some("settled-thread"));
    assert!(rebuilt.provider_fork_thread().is_none());
    assert!(rebuilt.resume_thread().is_none());
    assert!(rebuilt.delivers_history_with_turn());
    assert_eq!(rebuilt.checkpoint.confirmed_items, 0);
    assert_eq!(rebuilt.checkpoint.version, PREPARATION_CHECKPOINT_VERSION);
}
