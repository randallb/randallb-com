use super::*;

fn config(observation: Value) -> BoundedActionConfig {
    BoundedActionConfig {
        client_name: "history-test".into(),
        client_title: "History test".into(),
        model: None,
        reasoning_effort: None,
        workspace: PathBuf::from("/tmp"),
        service_tier_file: None,
        developer_instructions: "Preserve attributed work and corrections.".into(),
        legacy_context_window_tokens: 128 * 1024,
        observation,
        return_schema: json!({"type":"object"}),
        host_tools: vec![],
        model_run_ref: None,
        continuation: None,
        provider_store: None,
        ephemeral: false,
        native_text_result: false,
        provider_access: ProviderAccess::Isolated,
        sandbox: "read-only",
    }
}
fn observation() -> Value {
    json!({"context_snapshot":{"sha256":"source-one","message_count":3},
    "action_request":{"invocation_ref":"invocation-one","integration_ref":"actor-integration-one","params":{"exact":42}},
    "messages":[
        {"message_ref":"first","author":"coworker-a","text":"Use blue"},
        {"message_ref":"middle","author":"coworker-b","text":"Correction: use green"},
        {"message_ref":"last","reply_to":"first","inner_context":{"messages":[{"author":"private-helper","text":"Kept together"}]}}
    ]})
}
#[derive(Default)]
struct Handler {
    checkpoints: Vec<Value>,
    progress: Vec<(u16, String)>,
    phases: Vec<crate::adapter::ExecutionActivity>,
    timings: Vec<crate::adapter::AdapterTiming>,
}
impl BoundedActionHandler for Handler {
    fn on_timing(&mut self, timing: crate::adapter::AdapterTiming) {
        self.timings.push(timing);
    }
    fn on_progress(&mut self, status: u16, summary: &str) -> CwrResult<()> {
        self.progress.push((status, summary.to_owned()));
        Ok(())
    }
    fn on_activity(&mut self, activity: AdapterActivityEvent) -> CwrResult<()> {
        match activity {
            AdapterActivityEvent::ProviderEvent { data, .. } => self.checkpoints.push(data),
            AdapterActivityEvent::ExecutionActivity { activity } => self.phases.push(activity),
            _ => {}
        }
        Ok(())
    }
    fn on_model_output(&mut self, _: AdapterModelOutput) -> CwrResult<()> {
        Ok(())
    }
    fn on_host_call(&mut self, _: &str, _: &str, _: Value) -> CwrResult<ToolCallResult> {
        panic!("history must not execute tools")
    }
}

struct RejectCheckpoint;
impl BoundedActionHandler for RejectCheckpoint {
    fn on_progress(&mut self, _: u16, _: &str) -> CwrResult<()> {
        Ok(())
    }
    fn on_activity(&mut self, event: AdapterActivityEvent) -> CwrResult<()> {
        if matches!(event, AdapterActivityEvent::ExecutionActivity { .. }) {
            return Ok(());
        }
        Err(std::io::Error::from_raw_os_error(libc::EPERM).into())
    }
    fn on_model_output(&mut self, _: AdapterModelOutput) -> CwrResult<()> {
        Ok(())
    }
    fn on_host_call(&mut self, _: &str, _: &str, _: Value) -> CwrResult<ToolCallResult> {
        panic!("history must not execute tools")
    }
}
#[test]
fn source_turns_preserve_order_authors_replies_and_contained_inner_context() {
    let config = config(observation());
    let mut plan = HistoryPreparation::new(&config).unwrap();
    assert_eq!(plan.units.len(), 5);
    for (index, expected) in config.observation["messages"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let text = read_history_unit(&mut plan.spool, &plan.units[index + 2]).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), *expected);
    }
    let action = read_history_unit(&mut plan.spool, &plan.units[0]).unwrap();
    assert!(!action.contains("messages"));
    assert_eq!(plan.units[4].source_id, "requesting/2/last");
}

#[path = "history_projection_tests.rs"]
mod history_projection_tests;

#[path = "history_compaction_tests.rs"]
mod history_compaction_tests;

#[path = "history_capacity_tests.rs"]
mod history_capacity_tests;

#[test]
fn huge_individual_source_records_are_spooled_as_one_logical_turn_without_body_loss() {
    for mebibytes in [1, 4, 16] {
        let record = json!({"message_ref":"large","author":"helper","text":"界\"\\\n".repeat(mebibytes * 1024 * 1024 / 6)});
        let mut source = observation();
        source["messages"] = json!([record]);
        let mut plan = HistoryPreparation::new(&config(source)).unwrap();
        let mut complete = String::new();
        let parts = plan.units.len() - 2;
        for (part, unit) in plan.units[2..].iter().enumerate() {
            assert_eq!(unit.source_id, "requesting/0/large");
            assert_eq!((unit.part, unit.parts), (part, parts));
            assert!(unit.bytes <= HISTORY_PART_BYTES);
            complete.push_str(&read_history_unit(&mut plan.spool, unit).unwrap());
        }
        assert_eq!(serde_json::from_str::<Value>(&complete).unwrap(), record);
    }
}
#[test]
fn confirmed_context_resumes_without_reinjection_and_uncertain_context_reconstructs() {
    let mut config = config(observation());
    let mut plan = HistoryPreparation::new(&config).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    let mut handler = Handler::default();
    let mut writer = Vec::new();
    assert!(!plan.advance(&mut writer, &mut handler).unwrap());
    assert_eq!(handler.checkpoints.last().unwrap()["pending"], true);
    config.continuation = handler.checkpoints.last().cloned();
    let reconstructed = HistoryPreparation::new(&config).unwrap();
    assert!(reconstructed.resume_thread().is_none());
    assert_eq!(reconstructed.checkpoint.confirmed_items, 0);
    plan.injected(&mut handler).unwrap();
    config.continuation = handler.checkpoints.last().cloned();
    let resumed = HistoryPreparation::new(&config).unwrap();
    assert_eq!(resumed.resume_thread(), Some("native-one"));
    assert_eq!(resumed.checkpoint.confirmed_items, 1);
    config.observation["action_request"]["params"]["exact"] = json!(43);
    assert!(HistoryPreparation::new(&config).is_err());
}

#[test]
fn managed_store_survives_preparation_checkpoint_and_settled_history() {
    let mut config = config(observation());
    config.provider_store = Some("managed");
    config.observation["working_conversation"] = json!({"ref":"working-one"});
    let mut plan = HistoryPreparation::new(&config).unwrap();
    plan.checkpoint.thread_id = "managed-thread".into();
    let mut handler = Handler::default();
    plan.persist(&mut handler).unwrap();
    let checkpoint = handler.checkpoints.last().unwrap().clone();
    assert_eq!(checkpoint["provider_store"], "managed");
    config.continuation = Some(checkpoint);
    assert_eq!(
        HistoryPreparation::new(&config).unwrap().resume_thread(),
        Some("managed-thread")
    );

    plan.retain_working_state(&mut handler).unwrap();
    let state = handler.checkpoints.last().unwrap().clone();
    assert_eq!(state["version"], 3);
    assert_eq!(state["provider_store"], "managed");
    config.continuation = None;
    config.observation["working_conversation"]["state"] = state;
    assert_eq!(
        HistoryPreparation::new(&config)
            .unwrap()
            .prior_thread
            .as_deref(),
        Some("managed-thread")
    );

    config.provider_store = None;
    assert!(HistoryPreparation::new(&config).is_err());
    config.observation["working_conversation"]["state"]
        .as_object_mut()
        .unwrap()
        .remove("provider_store");
    assert_eq!(
        HistoryPreparation::new(&config)
            .unwrap()
            .prior_thread
            .as_deref(),
        Some("managed-thread")
    );
}

#[test]
fn system_store_survives_preparation_checkpoint_and_settled_history() {
    let mut config = config(observation());
    config.provider_store = Some("system");
    config.observation["working_conversation"] = json!({"ref":"working-system"});
    let mut plan = HistoryPreparation::new(&config).unwrap();
    plan.checkpoint.thread_id = "system-thread".into();
    let mut handler = Handler::default();
    plan.persist(&mut handler).unwrap();
    let checkpoint = handler.checkpoints.last().unwrap().clone();
    assert_eq!(checkpoint["provider_store"], "system");
    config.continuation = Some(checkpoint);
    assert_eq!(
        HistoryPreparation::new(&config).unwrap().resume_thread(),
        Some("system-thread")
    );

    plan.retain_working_state(&mut handler).unwrap();
    let state = handler.checkpoints.last().unwrap().clone();
    assert_eq!(state["provider_store"], "system");
    config.continuation = None;
    config.observation["working_conversation"]["state"] = state;
    assert_eq!(
        HistoryPreparation::new(&config)
            .unwrap()
            .prior_thread
            .as_deref(),
        Some("system-thread")
    );
}

#[test]
fn provider_store_checkpoint_must_match_the_launch_store() {
    let mut config = config(observation());
    let mut plan = HistoryPreparation::new(&config).unwrap();
    plan.checkpoint.thread_id = "legacy-thread".into();
    let mut handler = Handler::default();
    plan.persist(&mut handler).unwrap();
    let checkpoint = handler.checkpoints.last().unwrap().clone();
    assert!(checkpoint.get("provider_store").is_none());
    config.continuation = Some(checkpoint);
    assert_eq!(
        HistoryPreparation::new(&config).unwrap().resume_thread(),
        Some("legacy-thread")
    );
    config.provider_store = Some("managed");
    assert!(HistoryPreparation::new(&config).is_err());
}
#[test]
fn checkpoint_failures_name_the_history_boundary() {
    let mut plan = HistoryPreparation::new(&config(observation())).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    let error = plan
        .advance(&mut Vec::new(), &mut RejectCheckpoint)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("history checkpoint before unit 0 could not be retained"),
        "{error}"
    );
    assert!(
        error.to_string().contains("Operation not permitted"),
        "{error}"
    );
}
#[test]
fn history_restoration_reports_bounded_progress() {
    let mut source = observation();
    source["messages"] = json!((0..25)
        .map(|index| json!({"message_ref":format!("message-{index}"),"text":"history"}))
        .collect::<Vec<_>>());
    let mut plan = HistoryPreparation::new(&config(source)).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    let total = plan.units.len();
    let mut handler = Handler::default();
    let mut writer = Vec::new();
    while !plan.advance(&mut writer, &mut handler).unwrap() {
        writer.clear();
        plan.injected(&mut handler).unwrap();
    }
    use crate::adapter::{ExecutionPhase, ExecutionPhaseState};
    assert!(handler.progress.is_empty());
    assert_eq!(handler.phases.len(), 4);
    for (activity, done) in handler.phases.iter().zip([0, 10, 20, total as u64]) {
        assert_eq!(activity.phase, ExecutionPhase::RestoringContext);
        assert_eq!(activity.completed_parts, Some(done));
        assert_eq!(activity.total_parts, Some(total as u64));
    }
    assert_eq!(handler.phases[0].state, ExecutionPhaseState::Started);
    assert_eq!(handler.phases[3].state, ExecutionPhaseState::Completed);
}

#[test]
fn live_execution_is_never_replayed_as_context_reconstruction() {
    let mut config = config(observation());
    let mut plan = HistoryPreparation::new(&config).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    plan.checkpoint.live_started = true;
    config.continuation = Some(serde_json::to_value(&plan.checkpoint).unwrap());
    assert!(HistoryPreparation::new(&config)
        .err()
        .unwrap()
        .to_string()
        .contains("completed effects are preserved"));
}

#[test]
fn continuation_rejects_a_different_token_estimator() {
    let mut config = config(observation());
    let mut plan = HistoryPreparation::new(&config).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    let mut checkpoint = serde_json::to_value(&plan.checkpoint).unwrap();
    checkpoint["estimator"] = json!("tiktoken/other");
    config.continuation = Some(checkpoint);
    assert!(HistoryPreparation::new(&config)
        .err()
        .unwrap()
        .to_string()
        .contains("does not match"));
}

#[test]
fn supplied_requesting_context_has_individual_source_records_and_its_own_boundary() {
    let mut source = observation();
    source["requesting_context"] = json!({"conversation_ref":"parent","messages":[
        {"message_ref":"parent-first","author":"parent-coworker","text":"Parent constraint"}
    ]});
    let plan = HistoryPreparation::new(&config(source)).unwrap();
    assert_eq!(
        plan.units[5].source_id,
        "requesting/supplied-requesting-context/source-boundary"
    );
    assert_eq!(
        plan.units[6].source_id,
        "requesting/supplied-requesting-context/0/parent-first"
    );
}
#[test]
fn full_legacy_conversation_appends_and_preserves_compatibility() {
    let mut config = config(observation());
    let initial = HistoryPreparation::new(&config).unwrap();
    let current_digest = initial.checkpoint.initialization_digest.clone();
    // Build the legacy digest with the same inputs used by version 1.
    let legacy_digest = context_digest(&json!({
        "instructions":config.developer_instructions,"return_schema":config.return_schema,
        "model":config.model,"reasoning_effort":config.reasoning_effort,
        "context_window_tokens":config.legacy_context_window_tokens,"workspace":config.workspace,
        "native_text_result":config.native_text_result,"sandbox":config.sandbox,
        "provider_access":"isolated","tools":[]
    }))
    .unwrap();
    config.observation["working_conversation"] = json!({
        "ref":"working-one", "state":{
            "version":1, "provider":"codex", "idle":true,
            "working_ref":"working-one", "thread_id":"prior-thread",
            "initialization_digest":legacy_digest,"working_bytes":146662
        }
    });
    let mut plan = HistoryPreparation::new(&config).unwrap();
    assert_eq!(plan.prior_thread.as_deref(), Some("prior-thread"));
    assert_eq!(plan.provider_resume_thread(), Some("prior-thread"));
    assert!(plan.provider_fork_thread().is_none());
    assert!(plan.delivers_history_with_turn());
    assert_eq!(plan.checkpoint.initialization_digest, current_digest);
    plan.checkpoint.thread_id = "prior-thread".into();
    let mut handler = Handler::default();
    let mut encoded = Vec::new();
    assert!(plan.advance(&mut encoded, &mut handler).unwrap());
    assert!(encoded.is_empty());
    assert_eq!(plan.checkpoint.confirmed_items, plan.units.len());
    assert!(!plan.checkpoint.live_started);
    let input = plan
        .turn_input(&config.observation["action_request"])
        .unwrap();
    assert_eq!(input.as_array().unwrap().len(), plan.units.len() + 1);
    plan.begin_live_turn(&mut handler).unwrap();
    assert!(plan.checkpoint.live_started);
    plan.retain_working_state(&mut handler).unwrap();
    assert_eq!(handler.checkpoints.last().unwrap()["version"], 3);
    assert_eq!(
        handler.checkpoints.last().unwrap()["thread_id"],
        "prior-thread"
    );
    assert!(handler
        .checkpoints
        .last()
        .unwrap()
        .get("working_bytes")
        .is_none());

    // A legacy preparation can be reconstructed, but live work cannot replay.
    let mut legacy = serde_json::to_value(&plan.checkpoint).unwrap();
    legacy["version"] = json!(1);
    legacy["initialization_digest"] = json!(legacy_digest);
    legacy["live_started"] = json!(false);
    legacy["working_bytes"] = json!(146662);
    legacy["compacted_items"] = json!(0);
    legacy.as_object_mut().unwrap().remove("source_delivery");
    config.continuation = Some(legacy.clone());
    let reconstructed = HistoryPreparation::new(&config).unwrap();
    assert!(reconstructed.resume_thread().is_none());
    assert_eq!(reconstructed.provider_resume_thread(), Some("prior-thread"));
    assert!(reconstructed.delivers_history_with_turn());
    assert_eq!(reconstructed.checkpoint.confirmed_items, 0);
    legacy["live_started"] = json!(true);
    config.continuation = Some(legacy);
    assert!(HistoryPreparation::new(&config)
        .err()
        .unwrap()
        .to_string()
        .contains("completed effects"));
    config.continuation = None;
    config
        .developer_instructions
        .push_str("Changed instructions");
    assert!(HistoryPreparation::new(&config).is_err());
}

#[test]
fn source_items_are_attributed_user_messages_without_a_local_context_limit() {
    let mut source = observation();
    source["messages"] = json!([{"message_ref":"large","author":"other-coworker",
        "text":"x".repeat(64 * 1024)}]);
    let mut config = config(source);
    config.legacy_context_window_tokens = 16 * 1024;
    let mut plan = HistoryPreparation::new(&config).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    let mut handler = Handler::default();
    let mut encoded = Vec::new();
    for unit in 0..plan.units.len() {
        encoded.clear();
        assert!(!plan.advance(&mut encoded, &mut handler).unwrap());
        let request: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(request["method"], "thread/inject_items");
        let items = request["params"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["type"], "message");
        assert!(items[0]["id"].as_str().unwrap().starts_with("msg_"));
        assert_eq!(items[0]["role"], "user");
        let text = items[0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(&plan.units[unit].source_id));
        assert!(text.ends_with(&read_history_unit(&mut plan.spool, &plan.units[unit]).unwrap()));
        plan.injected(&mut handler).unwrap();
    }
    encoded.clear();
    assert!(plan.advance(&mut encoded, &mut handler).unwrap());
    assert!(encoded.is_empty());
}

#[test]
fn incompatible_private_history_reports_the_failed_condition() {
    let baseline = HistoryPreparation::new(&config(observation())).unwrap();
    let state = json!({
        "version":2, "provider":"codex", "idle":true,
        "working_ref":"working-one", "thread_id":"prior-thread",
        "initialization_digest":baseline.checkpoint.initialization_digest
    });
    for (field, value, reason) in [
        ("provider", json!("other"), "provider differs"),
        (
            "idle",
            json!(false),
            "previous provider conversation is not idle",
        ),
        (
            "working_ref",
            json!("other"),
            "working conversation reference differs",
        ),
        (
            "initialization_digest",
            json!("other"),
            "execution configuration digest differs",
        ),
    ] {
        let mut source = observation();
        source["working_conversation"] = json!({"ref":"working-one", "state":state});
        source["working_conversation"]["state"][field] = value;
        let error = HistoryPreparation::new(&config(source))
            .err()
            .expect("incompatible state must fail");
        assert!(error.to_string().contains(reason), "{error}");
    }
}
