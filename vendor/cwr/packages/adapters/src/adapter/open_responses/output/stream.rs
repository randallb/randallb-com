#[derive(Default)]
pub(super) struct StreamProjection {
    response_id: Option<String>,
    message_ids: BTreeSet<String>,
    reasoning_items: BTreeMap<String, ReasoningSummary>,
    emitted_text: bool,
}

struct ReasoningSummary {
    output_index: i64,
    parts: BTreeMap<i64, String>,
}

impl StreamProjection {
    pub fn observe(
        &mut self,
        output: &mut impl Write,
        invocation_ref: &str,
        model_run_ref: Option<&str>,
        event: &Value,
    ) -> CwrResult<Option<AdapterCompletedCommentary>> {
        let event_type = event.get("type").and_then(Value::as_str);
        if matches!(
            event_type,
            Some("response.created" | "response.in_progress")
        ) {
            if let Some(response_id) = event
                .pointer("/response/id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                self.response_id = Some(response_id.into());
            }
            return Ok(None);
        }

        match event_type {
            Some("response.output_item.added") => {
                let item = event.get("item");
                match item
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                {
                    Some("message")
                        if item
                            .and_then(|item| item.get("role"))
                            .and_then(Value::as_str)
                            == Some("assistant") =>
                    {
                        self.observe_message_added(output, invocation_ref, model_run_ref, event)?
                    }
                    Some("reasoning") => {
                        self.observe_reasoning_added(output, invocation_ref, model_run_ref, event)?
                    }
                    _ => {}
                }
            }
            Some(
                "response.content_part.added"
                | "response.content_part.done"
                | "response.output_text.delta"
                | "response.output_text.done",
            ) => self.observe_message_event(output, invocation_ref, model_run_ref, event)?,
            Some("response.reasoning_summary_text.delta") => {
                self.observe_reasoning_text(output, invocation_ref, model_run_ref, event, false)?
            }
            Some("response.reasoning_summary_text.done") => {
                self.observe_reasoning_text(output, invocation_ref, model_run_ref, event, true)?
            }
            Some("response.output_item.done") => {
                if event.pointer("/item/type").and_then(Value::as_str) == Some("reasoning") {
                    return self.observe_reasoning_done(
                        output,
                        invocation_ref,
                        model_run_ref,
                        event,
                    );
                }
                let Some(item_id) = event.pointer("/item/id").and_then(Value::as_str) else {
                    return Ok(None);
                };
                if self.message_ids.remove(item_id) {
                    self.emit_model_output(
                        output,
                        invocation_ref,
                        model_run_ref,
                        AssistantPhase::FinalAnswer,
                        with_phase(event, AssistantPhase::FinalAnswer),
                    )?;
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn observe_message_added(
        &mut self,
        output: &mut impl Write,
        invocation_ref: &str,
        model_run_ref: Option<&str>,
        event: &Value,
    ) -> CwrResult<()> {
        let Some(item_id) = event
            .pointer("/item/id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(());
        };
        self.message_ids.insert(item_id.into());
        self.emit_model_output(
            output,
            invocation_ref,
            model_run_ref,
            AssistantPhase::FinalAnswer,
            with_phase(event, AssistantPhase::FinalAnswer),
        )
    }

    fn observe_message_event(
        &mut self,
        output: &mut impl Write,
        invocation_ref: &str,
        model_run_ref: Option<&str>,
        event: &Value,
    ) -> CwrResult<()> {
        let Some(item_id) = event.get("item_id").and_then(Value::as_str) else {
            return Ok(());
        };
        if !self.message_ids.contains(item_id) {
            return Ok(());
        }
        self.emit_model_output(
            output,
            invocation_ref,
            model_run_ref,
            AssistantPhase::FinalAnswer,
            event.clone(),
        )?;
        if matches!(
            event.get("type").and_then(Value::as_str),
            Some("response.output_text.delta" | "response.output_text.done")
        ) {
            self.emitted_text = true;
        }
        Ok(())
    }

    fn observe_reasoning_added(
        &mut self,
        output: &mut impl Write,
        invocation_ref: &str,
        model_run_ref: Option<&str>,
        event: &Value,
    ) -> CwrResult<()> {
        let item_id = required_event_string(event, "/item/id", "reasoning item ID")?;
        let output_index = required_event_integer(event, "output_index")?;
        self.reasoning_items.insert(
            item_id.into(),
            ReasoningSummary {
                output_index,
                parts: BTreeMap::new(),
            },
        );
        let canonical = json!({
            "type": "response.output_item.added",
            "sequence_number": required_event_integer(event, "sequence_number")?,
            "output_index": output_index,
            "item": {
                "type": "message",
                "id": item_id,
                "status": "in_progress",
                "role": "assistant",
                "phase": "commentary",
                "content": [],
            },
        });
        self.emit_model_output(
            output,
            invocation_ref,
            model_run_ref,
            AssistantPhase::Commentary,
            canonical,
        )
    }

    fn observe_reasoning_text(
        &mut self,
        output: &mut impl Write,
        invocation_ref: &str,
        model_run_ref: Option<&str>,
        event: &Value,
        done: bool,
    ) -> CwrResult<()> {
        let item_id = required_event_string(event, "/item_id", "reasoning item ID")?;
        let Some(reasoning) = self.reasoning_items.get_mut(item_id) else {
            return Ok(());
        };
        let summary_index = required_event_integer(event, "summary_index")?;
        let field = if done { "text" } else { "delta" };
        let text = event
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| error(format!("Open Responses reasoning summary has no {field}")))?;
        if done {
            reasoning.parts.insert(summary_index, text.into());
        } else {
            reasoning
                .parts
                .entry(summary_index)
                .or_default()
                .push_str(text);
        }
        let sequence_number = required_event_integer(event, "sequence_number")?;
        let canonical = if done {
            json!({
                "type": "response.output_text.done",
                "sequence_number": sequence_number,
                "item_id": item_id,
                "output_index": reasoning.output_index,
                "content_index": summary_index,
                "text": text,
            })
        } else {
            json!({
                "type": "response.output_text.delta",
                "sequence_number": sequence_number,
                "item_id": item_id,
                "output_index": reasoning.output_index,
                "content_index": summary_index,
                "delta": text,
            })
        };
        self.emit_model_output(
            output,
            invocation_ref,
            model_run_ref,
            AssistantPhase::Commentary,
            canonical,
        )
    }

    fn observe_reasoning_done(
        &mut self,
        output: &mut impl Write,
        invocation_ref: &str,
        model_run_ref: Option<&str>,
        event: &Value,
    ) -> CwrResult<Option<AdapterCompletedCommentary>> {
        let item_id = required_event_string(event, "/item/id", "reasoning item ID")?;
        let Some(reasoning) = self.reasoning_items.remove(item_id) else {
            return Ok(None);
        };
        let text = reasoning_item_text(event.pointer("/item"))
            .unwrap_or_else(|| reasoning.parts.into_values().collect::<Vec<_>>().join("\n"));
        let canonical = json!({
            "type": "response.output_item.done",
            "sequence_number": required_event_integer(event, "sequence_number")?,
            "output_index": reasoning.output_index,
            "item": {
                "type": "message",
                "id": item_id,
                "status": "completed",
                "role": "assistant",
                "phase": "commentary",
                "content": [{"type":"output_text", "text":text, "annotations":[]}],
            },
        });
        self.emit_model_output(
            output,
            invocation_ref,
            model_run_ref,
            AssistantPhase::Commentary,
            canonical,
        )?;
        let (Some(model_run_ref), Some(response_id)) = (model_run_ref, self.response_id.as_deref())
        else {
            return Ok(None);
        };
        if text.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(AdapterCompletedCommentary {
            model_run_ref: model_run_ref.into(),
            provider: "open-responses".into(),
            thread_id: None,
            response_or_turn_id: response_id.into(),
            item_id: item_id.into(),
            text,
        }))
    }

    fn emit_model_output(
        &self,
        output: &mut impl Write,
        invocation_ref: &str,
        model_run_ref: Option<&str>,
        phase: AssistantPhase,
        event: Value,
    ) -> CwrResult<()> {
        let (Some(model_run_ref), Some(response_id)) = (model_run_ref, self.response_id.as_deref())
        else {
            return Ok(());
        };
        validate_event(&event).map_err(|cause| {
            error(format!(
                "Open Responses streaming model output is invalid: {cause}"
            ))
        })?;
        let message = AdapterEvent::ModelOutput {
            invocation_ref: invocation_ref.into(),
            output: AdapterModelOutput {
                model_run_ref: model_run_ref.into(),
                provider: "open-responses".into(),
                thread_id: None,
                response_or_turn_id: response_id.into(),
                phase,
                event,
            },
        };
        if serde_json::to_vec(&message)?.len() + 1 > MAX_PROTOCOL_LINE_BYTES {
            return Err(error(
                "Open Responses streaming output exceeds the adapter protocol line limit",
            ));
        }
        write_adapter_message(output, &message)?;
        Ok(())
    }

    pub fn emitted_text(&self) -> bool {
        self.emitted_text
    }
}

fn with_phase(event: &Value, phase: AssistantPhase) -> Value {
    let mut event = event.clone();
    if let Some(item) = event.get_mut("item").and_then(Value::as_object_mut) {
        item.insert("phase".into(), json!(phase.as_str()));
    }
    event
}

fn required_event_string<'a>(event: &'a Value, pointer: &str, label: &str) -> CwrResult<&'a str> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| error(format!("Open Responses stream has no {label}")))
}

fn required_event_integer(event: &Value, field: &str) -> CwrResult<i64> {
    event
        .get(field)
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| error(format!("Open Responses stream has no valid {field}")))
}

fn reasoning_item_text(item: Option<&Value>) -> Option<String> {
    let text = item?
        .get("summary")?
        .as_array()?
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("summary_text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}
