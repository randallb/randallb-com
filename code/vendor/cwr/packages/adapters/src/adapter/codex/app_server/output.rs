fn turn_started_scope(message: &Value) -> CwrResult<(String, String)> {
    let thread_id = message
        .pointer("/params/threadId")
        .and_then(Value::as_str)
        .and_then(nonblank)
        .ok_or("Codex turn/started notification has no thread ID")?;
    let turn_id = message
        .pointer("/params/turn/id")
        .and_then(Value::as_str)
        .and_then(nonblank)
        .ok_or("Codex turn/started notification has no turn ID")?;
    Ok((thread_id.into(), turn_id.into()))
}

fn establish_open_responses_scope(
    mapper: &mut Option<CodexOpenResponsesMapper>,
    thread_id: &str,
    turn_id: &str,
    source: &str,
) -> CwrResult<()> {
    if let Some(active) = mapper.as_ref() {
        if !active.matches_scope(thread_id, turn_id) {
            return invalid(format!(
                "Codex {source} scope {thread_id}/{turn_id} disagrees with active scope {}/{}",
                active.thread_id(),
                active.turn_id()
            ));
        }
    } else {
        *mapper = Some(CodexOpenResponsesMapper::new(thread_id, turn_id));
    }
    Ok(())
}

fn turn_notification_matches_scope(
    message: &Value,
    scope: Option<&CodexOpenResponsesMapper>,
) -> bool {
    let Some(scope) = scope else {
        return false;
    };
    let Some(turn_id) = message
        .pointer("/params/turn/id")
        .and_then(Value::as_str)
        .and_then(nonblank)
    else {
        return false;
    };
    if turn_id != scope.turn_id() {
        return false;
    }
    message
        .pointer("/params/threadId")
        .and_then(Value::as_str)
        .and_then(nonblank)
        .is_none_or(|thread_id| thread_id == scope.thread_id())
}

fn process_open_responses_message<H: BoundedActionHandler>(
    mapper: &mut CodexOpenResponsesMapper,
    message: &Value,
    received_at: Instant,
    model_run_ref: Option<&str>,
    include_final_answer: bool,
    timings: &mut TurnTimings,
    handler: &mut H,
) -> CwrResult<Option<CompletedAssistantMessage>> {
    if let Ok(mapped) = mapper.map_message(message) {
        if let Some(model_run_ref) = model_run_ref.and_then(nonblank) {
            for mapped_event in mapped.events {
                if mapped_event.phase == AssistantPhase::Commentary || include_final_answer {
                    let has_text = matches!(
                        mapped_event.event.get("type").and_then(Value::as_str),
                        Some("response.output_text.delta" | "response.output_text.done")
                    ) && mapped_event
                        .event
                        .get("delta")
                        .or_else(|| mapped_event.event.get("text"))
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty());
                    handler.on_model_output(AdapterModelOutput {
                        model_run_ref: model_run_ref.into(),
                        provider: "codex".into(),
                        thread_id: Some(mapper.thread_id().into()),
                        response_or_turn_id: mapper.turn_id().into(),
                        phase: mapped_event.phase,
                        event: mapped_event.event,
                    })?;
                    if has_text {
                        timings.text_received(received_at, handler);
                    }
                }
            }
        }
        if let Some(commentary) =
            completed_commentary(mapped.completed_assistant.clone(), model_run_ref)
        {
            handler.on_activity(AdapterActivityEvent::CompletedCommentary { commentary })?;
        }
        return Ok(mapped.completed_assistant.filter(|completed| {
            include_final_answer && completed.phase == AssistantPhase::FinalAnswer
        }));
    }
    Ok(None)
}

fn native_text_return(
    config: &BoundedActionConfig,
    completed: Option<CompletedAssistantMessage>,
) -> CwrResult<Value> {
    let completed = completed
        .ok_or_else(|| invalid_error("Codex action turn ended without a completed final answer"))?;
    let returned = json!({
        "status": 200,
        "summary": "Returned a response.",
        "result": {"text": completed.text}
    });
    validate_instance(
        "native text action return",
        &config.return_schema,
        &returned,
    )?;
    Ok(returned)
}

fn completed_commentary(
    completed: Option<CompletedAssistantMessage>,
    model_run_ref: Option<&str>,
) -> Option<AdapterCompletedCommentary> {
    let completed = completed?;
    if completed.phase != AssistantPhase::Commentary {
        return None;
    }
    if completed.text.trim().is_empty() {
        return None;
    }
    Some(AdapterCompletedCommentary {
        model_run_ref: nonblank(model_run_ref?)?.into(),
        provider: "codex".into(),
        thread_id: Some(completed.thread_id),
        response_or_turn_id: completed.turn_id,
        item_id: completed.item_id,
        text: completed.text,
    })
}

fn nonblank(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}
