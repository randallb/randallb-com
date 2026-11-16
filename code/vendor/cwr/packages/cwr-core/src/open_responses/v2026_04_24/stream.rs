#[derive(Debug)]
struct ItemState {
    output_index: i64,
    kind: OutputItemKind,
    phase: Option<AssistantPhase>,
    call_id: Option<String>,
    function_name: Option<String>,
    arguments: Option<String>,
    contents: HashMap<i64, ContentState>,
    closed: bool,
}

#[derive(Debug)]
struct ContentState {
    text: String,
    text_done: bool,
    closed: bool,
}

/// Validates event ordering and correlates item phase and text across an
/// interleaved provider stream.
#[derive(Debug, Default)]
pub struct StreamValidator {
    last_sequence: Option<i64>,
    items: HashMap<String, ItemState>,
    output_items: HashMap<i64, String>,
    closed: bool,
}

impl StreamValidator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn last_sequence_number(&self) -> Option<i64> {
        self.last_sequence
    }

    /// Validate and accept one event. State changes only after the event passes
    /// both pinned-schema and profile lifecycle validation.
    pub fn accept(&mut self, value: &Value) -> ProfileResult<ValidatedEvent> {
        if self.closed {
            return profile_failure("Open Responses stream is already closed");
        }
        let mut event = validate_event(value)?;
        if self
            .last_sequence
            .is_some_and(|last| event.sequence_number <= last)
        {
            return profile_failure(format!(
                "event sequence_number {} must be greater than the previous sequence {}",
                event.sequence_number,
                self.last_sequence.expect("checked above")
            ));
        }

        match event.kind {
            EventKind::OutputItemAdded => self.accept_item_added(&mut event)?,
            EventKind::OutputItemDone => self.accept_item_done(&mut event)?,
            EventKind::ContentPartAdded => self.accept_content_added(&mut event)?,
            EventKind::ContentPartDone => self.accept_content_done(&mut event)?,
            EventKind::OutputTextDelta => self.accept_text_delta(&mut event)?,
            EventKind::OutputTextDone => self.accept_text_done(&mut event)?,
            EventKind::Error => self.closed = true,
        }
        self.last_sequence = Some(event.sequence_number);
        Ok(event)
    }

    fn accept_item_added(&mut self, event: &mut ValidatedEvent) -> ProfileResult<()> {
        let item_id = event_item_id(event)?;
        let output_index = event_output_index(event)?;
        let kind = event
            .item_kind
            .ok_or_else(|| ProfileError::new("output-item event has no item kind"))?;
        if event.item_status != Some(ItemStatus::InProgress) {
            return profile_failure("response.output_item.added item must be in_progress");
        }
        if self.items.contains_key(item_id) {
            return profile_failure(format!("output item {item_id} was already added"));
        }
        if let Some(existing) = self.output_items.get(&output_index) {
            return profile_failure(format!(
                "output_index {output_index} already belongs to item {existing}"
            ));
        }
        self.items.insert(
            item_id.to_owned(),
            ItemState {
                output_index,
                kind,
                phase: event.phase,
                call_id: event.call_id.clone(),
                function_name: event.function_name.clone(),
                arguments: event.arguments.clone(),
                contents: HashMap::new(),
                closed: false,
            },
        );
        self.output_items.insert(output_index, item_id.to_owned());
        Ok(())
    }

    fn accept_item_done(&mut self, event: &mut ValidatedEvent) -> ProfileResult<()> {
        let item_id = event_item_id(event)?.to_owned();
        let output_index = event_output_index(event)?;
        let event_kind = event
            .item_kind
            .ok_or_else(|| ProfileError::new("output-item event has no item kind"))?;
        let event_status = event
            .item_status
            .ok_or_else(|| ProfileError::new("output-item event has no item status"))?;
        if !event_status.is_terminal() {
            return profile_failure("response.output_item.done item must be terminal");
        }
        let state = self
            .items
            .get(&item_id)
            .ok_or_else(|| ProfileError::new(format!("output item {item_id} was not added")))?;
        ensure_open_item(state, &item_id, output_index)?;
        if state.kind != event_kind {
            return profile_failure(format!(
                "output item {item_id} changed type before completion"
            ));
        }
        if state.phase != event.phase {
            return profile_failure(format!("output item {item_id} changed assistant phase"));
        }
        if event_kind == OutputItemKind::FunctionCall
            && (state.call_id != event.call_id
                || state.function_name != event.function_name
                || state.arguments != event.arguments)
        {
            return profile_failure(format!(
                "function-call output item {item_id} changed before completion"
            ));
        }
        if state.contents.values().any(|content| !content.closed) {
            return profile_failure(format!(
                "output item {item_id} completed before all content parts were done"
            ));
        }

        if event_kind == OutputItemKind::AssistantMessage {
            if event.text_parts.len() != state.contents.len() {
                return profile_failure(format!(
                    "output item {item_id} completed with content that did not follow the declared lifecycle"
                ));
            }
            for (index, completed_text) in event.text_parts.iter().enumerate() {
                let index = i64::try_from(index)
                    .map_err(|_| ProfileError::new("content index does not fit in an integer"))?;
                let content = state.contents.get(&index).ok_or_else(|| {
                    ProfileError::new(format!(
                        "output item {item_id} is missing completed content_index {index}"
                    ))
                })?;
                if &content.text != completed_text {
                    return profile_failure(format!(
                        "output item {item_id} content_index {index} disagrees with output_text.done"
                    ));
                }
            }
            event.text = Some(event.text_parts.concat());
            event.accumulated_text = event.text.clone();
        }

        let state = self.items.get_mut(&item_id).expect("checked above");
        event.phase = state.phase;
        state.closed = true;
        Ok(())
    }

    fn accept_content_added(&mut self, event: &mut ValidatedEvent) -> ProfileResult<()> {
        let item_id = event_item_id(event)?.to_owned();
        let output_index = event_output_index(event)?;
        let content_index = event_content_index(event)?;
        let state = self
            .items
            .get(&item_id)
            .ok_or_else(|| ProfileError::new(format!("output item {item_id} was not added")))?;
        ensure_open_item(state, &item_id, output_index)?;
        if state.kind != OutputItemKind::AssistantMessage {
            return profile_failure("content parts are supported only for assistant messages");
        }
        if state.contents.contains_key(&content_index) {
            return profile_failure(format!(
                "content_index {content_index} for item {item_id} was already added"
            ));
        }
        let initial_text = event.text.clone().unwrap_or_default();
        let phase = state.phase;
        self.items
            .get_mut(&item_id)
            .expect("checked above")
            .contents
            .insert(
                content_index,
                ContentState {
                    text: initial_text.clone(),
                    text_done: false,
                    closed: false,
                },
            );
        event.phase = phase;
        event.accumulated_text = Some(initial_text);
        Ok(())
    }

    fn accept_text_delta(&mut self, event: &mut ValidatedEvent) -> ProfileResult<()> {
        let (item_id, output_index, content_index) = event_coordinates(event)?;
        let phase = self.content_phase(item_id, output_index, content_index, false)?;
        let delta = event.delta.as_deref().unwrap_or_default();
        let content = self.content_mut(item_id, content_index);
        content.text.push_str(delta);
        event.phase = phase;
        event.accumulated_text = Some(content.text.clone());
        Ok(())
    }

    fn accept_text_done(&mut self, event: &mut ValidatedEvent) -> ProfileResult<()> {
        let (item_id, output_index, content_index) = event_coordinates(event)?;
        let phase = self.content_phase(item_id, output_index, content_index, false)?;
        let repaired = event.text.clone().unwrap_or_default();
        let content = self.content_mut(item_id, content_index);
        content.text = repaired.clone();
        content.text_done = true;
        event.phase = phase;
        event.accumulated_text = Some(repaired);
        Ok(())
    }

    fn accept_content_done(&mut self, event: &mut ValidatedEvent) -> ProfileResult<()> {
        let (item_id, output_index, content_index) = event_coordinates(event)?;
        let phase = self.content_phase(item_id, output_index, content_index, true)?;
        let completed_text = event.text.clone().unwrap_or_default();
        let content = self.content_mut(item_id, content_index);
        if content.text != completed_text {
            return profile_failure(format!(
                "content_index {content_index} for item {item_id} disagrees with output_text.done"
            ));
        }
        content.closed = true;
        event.phase = phase;
        event.accumulated_text = Some(completed_text);
        Ok(())
    }

    fn content_phase(
        &self,
        item_id: &str,
        output_index: i64,
        content_index: i64,
        require_text_done: bool,
    ) -> ProfileResult<Option<AssistantPhase>> {
        let state = self
            .items
            .get(item_id)
            .ok_or_else(|| ProfileError::new(format!("output item {item_id} was not added")))?;
        ensure_open_item(state, item_id, output_index)?;
        let content = state.contents.get(&content_index).ok_or_else(|| {
            ProfileError::new(format!(
                "content_index {content_index} for item {item_id} was not added"
            ))
        })?;
        if content.closed {
            return profile_failure(format!(
                "content_index {content_index} for item {item_id} is already closed"
            ));
        }
        if content.text_done && !require_text_done {
            return profile_failure(format!(
                "output text for content_index {content_index} of item {item_id} is already done"
            ));
        }
        if require_text_done && !content.text_done {
            return profile_failure(format!(
                "content_index {content_index} for item {item_id} closed before output_text.done"
            ));
        }
        Ok(state.phase)
    }

    fn content_mut(&mut self, item_id: &str, content_index: i64) -> &mut ContentState {
        self.items
            .get_mut(item_id)
            .expect("content_phase verified item")
            .contents
            .get_mut(&content_index)
            .expect("content_phase verified content")
    }
}

fn ensure_open_item(state: &ItemState, item_id: &str, output_index: i64) -> ProfileResult<()> {
    if state.closed {
        return profile_failure(format!("output item {item_id} is already closed"));
    }
    if state.output_index != output_index {
        return profile_failure(format!(
            "output item {item_id} belongs to output_index {}, not {output_index}",
            state.output_index
        ));
    }
    Ok(())
}

fn event_coordinates(event: &ValidatedEvent) -> ProfileResult<(&str, i64, i64)> {
    Ok((
        event_item_id(event)?,
        event_output_index(event)?,
        event_content_index(event)?,
    ))
}

fn event_item_id(event: &ValidatedEvent) -> ProfileResult<&str> {
    event
        .item_id
        .as_deref()
        .ok_or_else(|| ProfileError::new("event has no item_id"))
}

fn event_output_index(event: &ValidatedEvent) -> ProfileResult<i64> {
    event
        .output_index
        .ok_or_else(|| ProfileError::new("event has no output_index"))
}

fn event_content_index(event: &ValidatedEvent) -> ProfileResult<i64> {
    event
        .content_index
        .ok_or_else(|| ProfileError::new("event has no content_index"))
}
