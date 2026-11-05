use super::*;

#[test]
fn known_small_context_windows_keep_a_proportional_trigger() {
    let mut plan = HistoryPreparation::new(&config(observation())).unwrap();
    plan.checkpoint.latest_usage = Some(NativeTokenUsage {
        thread_id: "native-one".into(),
        turn_id: "turn-one".into(),
        total_tokens: 1,
        model_context_window: Some(16 * 1024),
    });
    plan.checkpoint.model_context_window = Some(16 * 1024);
    assert_eq!(plan.compaction_threshold(), 12 * 1024);
}

#[test]
fn stale_appended_usage_defers_capacity_handling_to_the_live_turn() {
    let mut source = observation();
    let digest = HistoryPreparation::new(&config(source.clone()))
        .unwrap()
        .checkpoint
        .initialization_digest;
    source["working_conversation"] = json!({
        "ref":"working-one",
        "state":{
            "version":3,
            "provider":"codex",
            "idle":true,
            "working_ref":"working-one",
            "thread_id":"prior-thread",
            "initialization_digest":digest,
            "invocation_ref":"prior-invocation",
            "context_snapshot":{},
            "estimator":cwr::token_estimation::ESTIMATOR,
            "last_total_tokens":63000,
            "model_context_window":65536,
            "usage_turn_id":"prior-turn",
            "usage_stale":true
        }
    });
    let config = config(source);
    let mut plan = HistoryPreparation::new(&config).unwrap();
    assert_eq!(plan.checkpoint.model_context_window, Some(65536));
    assert!(plan.checkpoint.latest_usage.is_none());
    assert!(plan.delivers_history_with_turn());
    plan.checkpoint.thread_id = "prior-thread".into();
    let mut handler = Handler::default();
    plan.report_oversized_fork(&mut handler);
    assert!(handler.timings.is_empty());
    let mut encoded = Vec::new();
    assert!(plan.advance(&mut encoded, &mut handler).unwrap());
    assert!(encoded.is_empty());
    assert_eq!(plan.checkpoint.confirmed_items, plan.units.len());
}

#[test]
fn oversized_settled_delta_uses_a_staged_fork_and_reports_it() {
    let mut source = observation();
    source["messages"] = json!([{"message_ref":"large","author":"coworker-a",
        "text":"x".repeat(1024 * 1024)}]);
    let baseline = HistoryPreparation::new(&config(source.clone())).unwrap();
    source["working_conversation"] = json!({
        "ref":"working-one",
        "state":{
            "version":3,
            "provider":"codex",
            "idle":true,
            "working_ref":"working-one",
            "thread_id":"prior-thread",
            "initialization_digest":baseline.checkpoint.initialization_digest,
            "invocation_ref":"prior-invocation",
            "context_snapshot":{},
            "estimator":cwr::token_estimation::ESTIMATOR,
            "usage_stale":true
        }
    });
    let plan = HistoryPreparation::new(&config(source)).unwrap();
    assert_eq!(
        plan.checkpoint.source_delivery,
        HistoryDelivery::OversizedFork
    );
    assert!(plan.provider_resume_thread().is_none());
    assert_eq!(plan.provider_fork_thread(), Some("prior-thread"));

    let mut handler = Handler::default();
    plan.report_oversized_fork(&mut handler);
    assert_eq!(handler.timings.len(), 1);
    assert_eq!(
        handler.timings[0].checkpoint,
        crate::adapter::TimingCheckpoint::OversizedHistoryFork
    );
}

#[test]
fn settled_thread_usage_keeps_the_current_live_request_budget() {
    let mut source = observation();
    let baseline = HistoryPreparation::new(&config(source.clone())).unwrap();
    let fixed_input_tokens = baseline.checkpoint.estimated_input_tokens;
    source["working_conversation"] = json!({
        "ref":"working-one",
        "state":{
            "version":3,
            "provider":"codex",
            "idle":true,
            "working_ref":"working-one",
            "thread_id":"prior-thread",
            "initialization_digest":baseline.checkpoint.initialization_digest,
            "invocation_ref":"prior-invocation",
            "context_snapshot":{},
            "estimator":cwr::token_estimation::ESTIMATOR,
            "last_total_tokens":63000,
            "model_context_window":65536,
            "usage_turn_id":"prior-turn",
            "usage_stale":false
        }
    });
    let plan = HistoryPreparation::new(&config(source)).unwrap();
    assert_eq!(
        plan.checkpoint.estimated_input_tokens,
        63000 + fixed_input_tokens
    );
    assert_eq!(plan.checkpoint.latest_usage.unwrap().total_tokens, 63000);
}

#[test]
fn live_usage_only_accepts_the_active_turn_and_terminal_settlement_marks_it_stale() {
    let mut plan = HistoryPreparation::new(&config(observation())).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    let usage = |turn_id| {
        json!({"method":"thread/tokenUsage/updated","params":{
        "threadId":"native-one","turnId":turn_id,"tokenUsage":{
            "last":{"totalTokens":55},"modelContextWindow":1000}}})
    };
    plan.observe_live_message(&usage("old-turn"), Some("live-turn"));
    assert!(plan.checkpoint.latest_usage.is_none());
    plan.observe_live_message(&usage("live-turn"), Some("live-turn"));
    assert_eq!(plan.checkpoint.estimated_input_tokens, 55);
    assert!(!plan.checkpoint.usage_stale);
    plan.mark_usage_stale();
    assert!(plan.checkpoint.usage_stale);
}

#[test]
fn native_usage_is_scoped_to_the_active_thread() {
    let mut plan = HistoryPreparation::new(&config(observation())).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    let original = plan.checkpoint.estimated_input_tokens;
    plan.observe_token_usage(&json!({"method":"thread/tokenUsage/updated","params":{
        "threadId":"other-thread","turnId":"turn-one","tokenUsage":{
            "last":{"totalTokens":55},"modelContextWindow":1000}}}));
    assert_eq!(plan.checkpoint.estimated_input_tokens, original);
    assert!(plan.checkpoint.latest_usage.is_none());
    plan.observe_token_usage(&json!({"method":"thread/tokenUsage/updated","params":{
        "threadId":"native-one","turnId":"turn-one","tokenUsage":{
            "last":{"totalTokens":55},"modelContextWindow":1000}}}));
    assert_eq!(plan.checkpoint.estimated_input_tokens, 55);
    assert_eq!(
        plan.checkpoint.latest_usage.unwrap().model_context_window,
        Some(1000)
    );
}
