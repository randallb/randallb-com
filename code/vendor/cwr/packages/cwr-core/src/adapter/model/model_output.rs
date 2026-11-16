#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct ModelOutputItemKey {
    invocation_ref: String,
    model_run_ref: String,
    provider: String,
    thread_id: Option<String>,
    response_or_turn_id: String,
    item_id: String,
}

#[derive(Debug, Default)]
struct AdapterModelOutputMailboxState {
    revision: u64,
    disabled: bool,
    visible: Option<AdapterLiveOutput>,
    visible_clipped: bool,
    pending_bytes: usize,
    last_sequences: BTreeMap<(String, String), i64>,
    open_items: BTreeSet<ModelOutputItemKey>,
    closed_items: BTreeSet<ModelOutputItemKey>,
    closed_runs: BTreeSet<(String, String)>,
}

/// Latest-state, byte-bounded delivery for temporary model text.
///
/// Adapter stdout folds model-output events into this mailbox before any
/// presenter can apply backpressure. Only one current assistant-text snapshot
/// is retained; durable completion and run closure supersede it.
#[derive(Debug, Clone)]
pub struct AdapterModelOutputMailbox {
    byte_limit: usize,
    state: Arc<Mutex<AdapterModelOutputMailboxState>>,
}

impl Default for AdapterModelOutputMailbox {
    fn default() -> Self {
        Self::with_byte_limit(MODEL_OUTPUT_MAILBOX_BYTES)
    }
}

impl AdapterModelOutputMailbox {
    pub fn with_byte_limit(byte_limit: usize) -> Self {
        Self {
            byte_limit,
            state: Arc::new(Mutex::new(AdapterModelOutputMailboxState::default())),
        }
    }

    pub fn discard() -> Self {
        Self::with_byte_limit(0)
    }

    pub fn snapshot(&self) -> AdapterLiveOutputSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        AdapterLiveOutputSnapshot {
            revision: state.revision,
            pending_bytes: state.pending_bytes,
            output: state.visible.clone().map(|mut output| {
                if state.visible_clipped {
                    output.text.insert_str(0, MODEL_OUTPUT_CLIPPED);
                }
                output
            }),
            closed_model_runs: state
                .closed_runs
                .iter()
                .map(|(invocation_ref, model_run_ref)| AdapterClosedModelRun {
                    invocation_ref: invocation_ref.clone(),
                    model_run_ref: model_run_ref.clone(),
                })
                .collect(),
        }
    }

    fn accept(
        &self,
        expected_invocation_ref: &str,
        expected_model_run_ref: Option<&str>,
        invocation_ref: String,
        output: AdapterModelOutput,
    ) -> CwrResult<()> {
        use crate::open_responses::v2026_04_24::{
            validate_event, AssistantPhase, EventKind, OutputItemKind,
        };

        if invocation_ref != expected_invocation_ref {
            return protocol_failure(
                "adapter model output invocation ref does not match the request",
            );
        }
        let expected_model_run_ref = expected_model_run_ref.ok_or_else(|| {
            protocol_error("adapter emitted model output without a claimed model run")
        })?;
        if output.model_run_ref != expected_model_run_ref {
            return protocol_failure(format!(
                "adapter model output run {} does not match claimed run {expected_model_run_ref}",
                output.model_run_ref
            ));
        }
        if output.provider.trim().is_empty() || output.response_or_turn_id.trim().is_empty() {
            return protocol_failure("adapter model output identity must not be blank");
        }
        if output
            .thread_id
            .as_deref()
            .is_some_and(|thread_id| thread_id.trim().is_empty())
        {
            return protocol_failure("adapter model output thread ID must not be blank");
        }

        let event = validate_event(&output.event).map_err(|error| {
            protocol_error(format!("adapter model output event is invalid: {error}"))
        })?;
        if event.kind != EventKind::Error
            && event.phase.is_some_and(|phase| phase != output.phase)
        {
            return protocol_failure("adapter model output phase disagrees with its event");
        }
        let item_id = if event.kind == EventKind::Error {
            None
        } else {
            if event
                .item_kind
                .is_some_and(|kind| kind != OutputItemKind::AssistantMessage)
            {
                return Ok(());
            }
            let Some(item_id) = event.item_id.clone() else {
                return Ok(());
            };
            Some(item_id)
        };
        let run_key = (invocation_ref.clone(), output.model_run_ref.clone());
        let scope_key = (
            output.model_run_ref.clone(),
            output.response_or_turn_id.clone(),
        );
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.disabled || state.closed_runs.contains(&run_key) {
            return Ok(());
        }
        if event.kind == EventKind::Error {
            self.close_run_locked(&mut state, &run_key);
            self.enforce_byte_limit(&mut state);
            return Ok(());
        }
        if state
            .last_sequences
            .get(&scope_key)
            .is_some_and(|last| event.sequence_number <= *last)
        {
            return Ok(());
        }
        state
            .last_sequences
            .insert(scope_key, event.sequence_number);
        let item_id = item_id.expect("non-error model output must have an item ID");
        let key = ModelOutputItemKey {
            invocation_ref: invocation_ref.clone(),
            model_run_ref: output.model_run_ref.clone(),
            provider: output.provider.clone(),
            thread_id: output.thread_id.clone(),
            response_or_turn_id: output.response_or_turn_id.clone(),
            item_id: item_id.clone(),
        };
        if state.closed_items.contains(&key) {
            self.enforce_byte_limit(&mut state);
            return Ok(());
        }

        match event.kind {
            EventKind::OutputItemAdded => {
                if event.item_kind == Some(OutputItemKind::AssistantMessage) {
                    state.open_items.insert(key);
                }
            }
            EventKind::ContentPartAdded => {
                if state.open_items.contains(&key) {
                    if let Some(text) = event.text.filter(|text| !text.is_empty()) {
                        self.replace_visible(&mut state, live_output(
                            invocation_ref,
                            output,
                            item_id,
                            event.sequence_number,
                            text,
                        ), false);
                    }
                }
            }
            EventKind::OutputTextDelta => {
                if state.open_items.contains(&key) {
                    let delta = event.delta.unwrap_or_default();
                    if !delta.is_empty() {
                        let clipped = state.visible_clipped && state.visible.as_ref()
                            .is_some_and(|visible| model_output_key(visible) == key);
                        let text = state
                            .visible
                            .as_ref()
                            .filter(|visible| model_output_key(visible) == key)
                            .map(|visible| {
                                let mut text = visible.text.clone();
                                text.push_str(&delta);
                                text
                            })
                            .unwrap_or(delta);
                        self.replace_visible(&mut state, live_output(
                            invocation_ref,
                            output,
                            item_id,
                            event.sequence_number,
                            text,
                        ), clipped);
                    }
                }
            }
            EventKind::OutputTextDone | EventKind::ContentPartDone => {
                if state.open_items.contains(&key) {
                    if let Some(text) = event.text {
                        self.replace_visible(&mut state, live_output(
                            invocation_ref,
                            output,
                            item_id,
                            event.sequence_number,
                            text,
                        ), false);
                    }
                }
            }
            EventKind::OutputItemDone => {
                state.open_items.remove(&key);
                state.closed_items.insert(key.clone());
                if output.phase == AssistantPhase::Commentary
                    && state
                    .visible
                    .as_ref()
                    .is_some_and(|visible| model_output_key(visible) == key)
                {
                    clear_visible(&mut state);
                }
            }
            EventKind::Error => unreachable!("Open Responses error handled before item identity"),
        }
        self.enforce_byte_limit(&mut state);
        Ok(())
    }

    fn replace_visible(
        &self,
        state: &mut AdapterModelOutputMailboxState,
        output: AdapterLiveOutput,
        clipped: bool,
    ) {
        if state.visible.as_ref() == Some(&output) && state.visible_clipped == clipped {
            return;
        }
        state.visible = Some(output);
        state.visible_clipped = clipped;
        state.revision = state.revision.wrapping_add(1);
    }

    fn complete_commentary(
        &self,
        invocation_ref: &str,
        commentary: &AdapterCompletedCommentary,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.disabled {
            return;
        }
        let key = ModelOutputItemKey {
            invocation_ref: invocation_ref.into(),
            model_run_ref: commentary.model_run_ref.clone(),
            provider: commentary.provider.clone(),
            thread_id: commentary.thread_id.clone(),
            response_or_turn_id: commentary.response_or_turn_id.clone(),
            item_id: commentary.item_id.clone(),
        };
        state.open_items.remove(&key);
        state.closed_items.insert(key.clone());
        if state
            .visible
            .as_ref()
            .is_some_and(|visible| model_output_key(visible) == key)
        {
            clear_visible(&mut state);
        }
        self.enforce_byte_limit(&mut state);
    }

    fn close_run(&self, invocation_ref: &str, model_run_ref: Option<&str>) {
        let Some(model_run_ref) = model_run_ref else {
            return;
        };
        let run_key = (invocation_ref.to_owned(), model_run_ref.to_owned());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.disabled {
            return;
        }
        self.close_run_locked(&mut state, &run_key);
        self.enforce_byte_limit(&mut state);
    }

    fn close_run_locked(
        &self,
        state: &mut AdapterModelOutputMailboxState,
        run_key: &(String, String),
    ) {
        if state.closed_runs.insert(run_key.clone()) {
            state.revision = state.revision.wrapping_add(1);
        }
        state
            .open_items
            .retain(|key| (&key.invocation_ref, &key.model_run_ref) != (&run_key.0, &run_key.1));
        state
            .closed_items
            .retain(|key| (&key.invocation_ref, &key.model_run_ref) != (&run_key.0, &run_key.1));
        state
            .last_sequences
            .retain(|(model_run_ref, _), _| model_run_ref != &run_key.1);
        if state.visible.as_ref().is_some_and(|visible| {
            visible.invocation_ref == run_key.0 && visible.model_run_ref == run_key.1
        }) {
            clear_visible(state);
        }
    }

    fn enforce_byte_limit(&self, state: &mut AdapterModelOutputMailboxState) {
        let measured = model_output_state_bytes(state)
            .unwrap_or_else(|| self.byte_limit.saturating_add(1));
        if measured <= self.byte_limit {
            state.pending_bytes = measured;
            return;
        }

        if let Some(visible) = state.visible.as_mut() {
            // Reserve identities, sequence numbers and tombstones first. Count
            // JSON escaping and the marker as well as the retained UTF-8 text.
            let text = std::mem::take(&mut visible.text);
            state.visible_clipped = true;
            if let Some(available) = model_output_state_bytes(state)
                .and_then(|metadata| self.byte_limit.checked_sub(metadata))
            {
                state.visible.as_mut().expect("visible output").text =
                    model_output_tail(&text, available).to_owned();
                state.pending_bytes = model_output_state_bytes(state).expect("serializable output");
                state.revision = state.revision.wrapping_add(1);
                return;
            }
            clear_visible(state);
            if let Some(measured) = model_output_state_bytes(state)
                .filter(|measured| *measured <= self.byte_limit)
            {
                state.pending_bytes = measured;
                return;
            }
        }

        // Only lifecycle metadata exhaustion disables delivery. Evicting its
        // sequence or closure records could let late output revive closed work.
        clear_visible(state);
        state.last_sequences.clear();
        state.open_items.clear();
        state.closed_items.clear();
        state.closed_runs.clear();
        state.pending_bytes = 0;
        state.disabled = true;
    }
}

fn model_output_state_bytes(state: &AdapterModelOutputMailboxState) -> Option<usize> {
    if state.visible.is_none()
        && state.last_sequences.is_empty()
        && state.open_items.is_empty()
        && state.closed_items.is_empty()
        && state.closed_runs.is_empty()
    {
        return Some(0);
    }
    let last_sequences = state
        .last_sequences
        .iter()
        .map(|((model_run_ref, response_or_turn_id), sequence)| {
            (model_run_ref, response_or_turn_id, sequence)
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&(
        &state.visible,
        last_sequences,
        &state.open_items,
        &state.closed_items,
        &state.closed_runs,
    ))
    .ok()
    .map(|bytes| bytes.len() + if state.visible_clipped {
        MODEL_OUTPUT_CLIPPED.chars().map(model_output_char_bytes).sum::<usize>()
    } else {
        0
    })
}

const MODEL_OUTPUT_CLIPPED: &str = "[earlier output truncated]\n";

fn model_output_tail(text: &str, byte_limit: usize) -> &str {
    let mut bytes = 0;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        bytes += model_output_char_bytes(character);
        if bytes > byte_limit {
            break;
        }
        start = index;
    }
    &text[start..]
}

fn model_output_char_bytes(character: char) -> usize {
    match character {
        '"' | '\\' | '\u{0008}' | '\u{000c}' | '\n' | '\r' | '\t' => 2,
        '\u{0000}'..='\u{001f}' => 6,
        _ => character.len_utf8(),
    }
}

fn live_output(
    invocation_ref: String,
    output: AdapterModelOutput,
    item_id: String,
    sequence_number: i64,
    text: String,
) -> AdapterLiveOutput {
    AdapterLiveOutput {
        invocation_ref,
        model_run_ref: output.model_run_ref,
        provider: output.provider,
        thread_id: output.thread_id,
        response_or_turn_id: output.response_or_turn_id,
        item_id,
        phase: output.phase,
        sequence_number,
        text,
    }
}

fn model_output_key(output: &AdapterLiveOutput) -> ModelOutputItemKey {
    ModelOutputItemKey {
        invocation_ref: output.invocation_ref.clone(),
        model_run_ref: output.model_run_ref.clone(),
        provider: output.provider.clone(),
        thread_id: output.thread_id.clone(),
        response_or_turn_id: output.response_or_turn_id.clone(),
        item_id: output.item_id.clone(),
    }
}

fn clear_visible(state: &mut AdapterModelOutputMailboxState) {
    state.visible_clipped = false;
    if state.visible.take().is_some() {
        state.pending_bytes = 0;
        state.revision = state.revision.wrapping_add(1);
    }
}
