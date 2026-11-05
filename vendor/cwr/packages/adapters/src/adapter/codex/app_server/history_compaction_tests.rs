use super::*;

#[test]
fn failed_preparation_compaction_has_a_terminal_timing_observation() {
    let mut plan = HistoryPreparation::new(&config(observation())).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    plan.checkpoint.compaction = CompactionState::Running {
        turn_id: "compact-one".into(),
        request_accepted: true,
        item_completed: false,
        turn_completed: false,
    };
    let started = Instant::now();
    plan.compaction_started_at = Some(started);
    let mut handler = Handler::default();
    let error = plan
        .observe_compaction(
            &json!({"method":"turn/completed","params":{
                "threadId":"native-one",
                "turn":{"id":"compact-one","status":"failed","error":{"code":"failed"}}}}),
            started + Duration::from_millis(5),
            &mut handler,
        )
        .unwrap_err();
    assert!(error.to_string().contains("context compaction failed"));
    assert_eq!(handler.timings.len(), 1);
    let timing = &handler.timings[0];
    assert_eq!(
        timing.checkpoint,
        crate::adapter::TimingCheckpoint::PreparationCompactionFailed
    );
    assert_eq!(
        timing.compaction.as_ref().unwrap().outcome,
        crate::adapter::AdapterCompactionOutcome::Failed
    );
    assert_eq!(
        timing.durations_ms[&crate::adapter::TimingDuration::ContextCompaction],
        5
    );
}

#[test]
fn preparation_compaction_waits_for_both_item_and_turn_in_either_order() {
    let mut plan = HistoryPreparation::new(&config(observation())).unwrap();
    let live_input_tokens = plan.live_input_tokens;
    plan.checkpoint.thread_id = "native-one".into();
    plan.checkpoint.latest_usage = Some(NativeTokenUsage {
        thread_id: "native-one".into(),
        turn_id: "compact-one".into(),
        total_tokens: 100,
        model_context_window: Some(1000),
    });
    plan.checkpoint.compaction = CompactionState::Running {
        turn_id: "compact-one".into(),
        request_accepted: true,
        item_completed: false,
        turn_completed: false,
    };
    let mut handler = Handler::default();
    let turn = json!({"method":"turn/completed","params":{"threadId":"native-one",
        "turn":{"id":"compact-one","status":"completed"}}});
    assert!(!plan
        .observe_compaction(&turn, Instant::now(), &mut handler)
        .unwrap());
    assert!(!plan.checkpoint.compaction.is_idle());
    let item = json!({"method":"item/completed","params":{"threadId":"native-one",
        "turnId":"compact-one","item":{"type":"contextCompaction","id":"item-one"}}});
    assert!(plan
        .observe_compaction(&item, Instant::now(), &mut handler)
        .unwrap());
    assert!(plan.checkpoint.compaction.is_idle());
    assert_eq!(plan.checkpoint.compactions, 1);
    assert_eq!(
        plan.checkpoint.estimated_input_tokens,
        100 + live_input_tokens
    );
}

#[test]
fn preparation_compaction_waits_for_late_request_acceptance() {
    let mut plan = HistoryPreparation::new(&config(observation())).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    plan.checkpoint.compaction = CompactionState::Running {
        turn_id: "compact-one".into(),
        request_accepted: false,
        item_completed: true,
        turn_completed: true,
    };
    let mut handler = Handler::default();
    assert!(plan
        .compaction_request_accepted(Instant::now(), &mut handler)
        .unwrap());
    assert!(plan.checkpoint.compaction.is_idle());
    assert_eq!(plan.checkpoint.compactions, 1);
}

#[test]
fn preparation_compaction_reports_scoped_phases_and_resumes_restoration_immediately() {
    use crate::adapter::{ExecutionPhase, ExecutionPhaseState};

    let mut plan = HistoryPreparation::new(&config(observation())).unwrap();
    plan.checkpoint.thread_id = "native-one".into();
    plan.checkpoint.compaction = CompactionState::Requested {
        request_accepted: true,
    };
    let mut handler = Handler::default();
    plan.report_progress(&mut handler).unwrap();

    let unrelated_turn = json!({"method":"turn/started","params":{"threadId":"other-thread",
        "turn":{"id":"other-turn"}}});
    assert!(!plan
        .observe_compaction(&unrelated_turn, Instant::now(), &mut handler)
        .unwrap());
    assert_eq!(handler.phases.len(), 1);

    let turn_started = json!({"method":"turn/started","params":{"threadId":"native-one",
        "turn":{"id":"compact-one"}}});
    assert!(!plan
        .observe_compaction(&turn_started, Instant::now(), &mut handler)
        .unwrap());
    assert_eq!(
        handler.phases.last().unwrap().phase,
        ExecutionPhase::CompactingContext
    );
    assert_eq!(
        handler.phases.last().unwrap().state,
        ExecutionPhaseState::Started
    );

    let unrelated_item = json!({"method":"item/completed","params":{"threadId":"native-one",
        "turnId":"other-turn","item":{"type":"contextCompaction"}}});
    assert!(!plan
        .observe_compaction(&unrelated_item, Instant::now(), &mut handler)
        .unwrap());
    assert_eq!(handler.phases.len(), 2);

    let item = json!({"method":"item/completed","params":{"threadId":"native-one",
        "turnId":"compact-one","item":{"type":"contextCompaction"}}});
    assert!(!plan
        .observe_compaction(&item, Instant::now(), &mut handler)
        .unwrap());
    let turn_completed = json!({"method":"turn/completed","params":{"threadId":"native-one",
        "turn":{"id":"compact-one","status":"completed"}}});
    assert!(plan
        .observe_compaction(&turn_completed, Instant::now(), &mut handler)
        .unwrap());
    assert_eq!(
        handler.phases.last().unwrap().phase,
        ExecutionPhase::CompactingContext
    );
    assert_eq!(
        handler.phases.last().unwrap().state,
        ExecutionPhaseState::Completed
    );

    assert!(!plan.advance(&mut Vec::new(), &mut handler).unwrap());
    let restoration = handler.phases.last().unwrap();
    assert_eq!(restoration.phase, ExecutionPhase::RestoringContext);
    assert_eq!(restoration.state, ExecutionPhaseState::Updated);
    assert_eq!(restoration.completed_parts, Some(0));
}
