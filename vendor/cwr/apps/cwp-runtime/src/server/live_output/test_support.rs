fn started(invocation_ref: &str) -> LocalRuntimeEvent {
    LocalRuntimeEvent::InvocationStarted {
        invocation_ref: invocation_ref.into(),
        generation: 1,
        target_actor_ref: target_ref().as_str().into(),
    }
}

fn commentary(invocation_ref: &str, item_id: &str, text: &str) -> LocalRuntimeEvent {
    LocalRuntimeEvent::Activity {
        invocation_ref: invocation_ref.into(),
        target_actor_ref: target_ref().as_str().into(),
        activity: Box::new(ConversationActivity {
            activity_ref: format!("cwp://example.test/runtime/activities/{item_id}"),
            sequence: 1,
            occurred_at_unix_ms: 1,
            event: ConversationActivityEvent::CommentaryProgress {
                model_run_ref: "cwp://example.test/runtime/model_runs/one".into(),
                provider_integration_ref: "~@/integrations/codex".into(),
                provider: "codex".into(),
                thread_id: Some("thread-one".into()),
                response_or_turn_id: "turn-one".into(),
                item_id: item_id.into(),
                text: text.into(),
            },
        }),
    }
}

fn terminal(invocation_ref: &str) -> LocalRuntimeEvent {
    LocalRuntimeEvent::Terminal {
        target_actor_ref: target_ref().as_str().into(),
        response: Box::new(cwr::v2026_09::invocation::ActionResponse {
            progress: None,
            invocation_ref: invocation_ref.into(),
            sequence: 1,
            status: cwr::v2026_09::invocation::ActionResponseStatus::COMPLETED,
            summary: "Done".into(),
            result: Some(serde_json::json!({})),
            error: None,
            cause: None,
        }),
        message: None,
    }
}

fn output(invocation_ref: &str, item_id: &str, text: &str) -> AdapterLiveOutput {
    AdapterLiveOutput {
        invocation_ref: invocation_ref.into(),
        model_run_ref: "cwp://example.test/runtime/model_runs/one".into(),
        provider: "codex".into(),
        thread_id: Some("thread-one".into()),
        response_or_turn_id: "turn-one".into(),
        item_id: item_id.into(),
        phase: AssistantPhase::Commentary,
        sequence_number: 1,
        text: text.into(),
    }
}

fn projected(invocation_ref: &str, text: &str) -> ConversationLiveOutput {
    ConversationLiveOutput {
        invocation_ref: invocation_ref.into(),
        generation: 1,
        target_actor_ref: target_ref().as_str().into(),
        phase: AssistantPhase::Commentary,
        text: text.into(),
    }
}
