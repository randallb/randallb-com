use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use cwr::{
    adapter::{AdapterClosedModelRun, AdapterLiveOutput, AdapterModelOutputMailbox},
    open_responses::v2026_04_24::AssistantPhase,
    v2026_09::{
        conversation::ConversationActivityEvent,
        host::ConversationLiveOutput,
        local::{LocalRuntime, LocalRuntimeEvent},
    },
    CwrResult,
};

use super::*;

const LIVE_OUTPUT_FLUSH_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct LiveOutputItemKey {
    invocation_ref: String,
    model_run_ref: String,
    provider: String,
    thread_id: Option<String>,
    response_or_turn_id: String,
    item_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InvocationContext {
    conversation_ref: String,
    generation: u64,
    target_actor_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InvocationLiveOutput {
    item_key: LiveOutputItemKey,
    output: ConversationLiveOutput,
    update_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LiveOutputChange {
    pub(super) conversation_ref: String,
    pub(super) output: Option<ConversationLiveOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RequiredLiveOutputChange {
    delivery_sequence: u64,
    change: LiveOutputChange,
}

#[derive(Debug, Default)]
pub(super) struct LiveOutputRegistry {
    contexts: BTreeMap<String, InvocationContext>,
    by_invocation: BTreeMap<String, InvocationLiveOutput>,
    presented_by_conversation: BTreeMap<String, ConversationLiveOutput>,
    completed_items: BTreeSet<LiveOutputItemKey>,
    required_changes: VecDeque<RequiredLiveOutputChange>,
    update_sequence: u64,
    delivery_sequence: u64,
}

impl LiveOutputRegistry {
    pub(super) fn snapshot(&self) -> Vec<(String, ConversationLiveOutput)> {
        self.presented_by_conversation
            .iter()
            .map(|(conversation_ref, output)| (conversation_ref.clone(), output.clone()))
            .collect()
    }

    pub(super) fn broadcast_snapshot(
        &self,
    ) -> (
        Vec<RequiredLiveOutputChange>,
        BTreeMap<String, ConversationLiveOutput>,
    ) {
        (
            self.required_changes.iter().cloned().collect(),
            self.snapshot().into_iter().collect(),
        )
    }

    fn acknowledge_required_changes(&mut self, through_sequence: u64) {
        while self
            .required_changes
            .front()
            .is_some_and(|change| change.delivery_sequence <= through_sequence)
        {
            self.required_changes.pop_front();
        }
    }

    fn observe_runtime_event(
        &mut self,
        conversation_ref: &str,
        event: &LocalRuntimeEvent,
    ) -> Vec<LiveOutputChange> {
        let (invocation_ref, generation, target_actor_ref) = runtime_event_identity(event);
        let mut affected = self.register_context(
            invocation_ref,
            conversation_ref.to_owned(),
            generation,
            target_actor_ref.to_owned(),
        );
        match event {
            LocalRuntimeEvent::Activity { activity, .. } => {
                if let ConversationActivityEvent::CommentaryProgress {
                    model_run_ref,
                    provider,
                    thread_id,
                    response_or_turn_id,
                    item_id,
                    text,
                    ..
                } = &activity.event
                {
                    self.complete_commentary(
                        invocation_ref,
                        LiveOutputItemKey {
                            invocation_ref: invocation_ref.into(),
                            model_run_ref: model_run_ref.clone(),
                            provider: provider.clone(),
                            thread_id: thread_id.clone(),
                            response_or_turn_id: response_or_turn_id.clone(),
                            item_id: item_id.clone(),
                        },
                        text,
                        &mut affected,
                    );
                }
            }
            LocalRuntimeEvent::Terminal { .. } => {
                self.remove_invocation(invocation_ref, &mut affected);
            }
            LocalRuntimeEvent::InvocationStarted { .. } => {}
        }
        self.recompute(affected)
    }

    fn apply_stream_snapshot(
        &mut self,
        previous: Option<&AdapterLiveOutput>,
        current: Option<&AdapterLiveOutput>,
        closed_model_runs: &[AdapterClosedModelRun],
    ) -> Vec<LiveOutputChange> {
        let mut affected = BTreeSet::new();
        if let Some(previous) = previous.filter(|previous| {
            current.is_none_or(|current| live_output_key(previous) != live_output_key(current))
        }) {
            self.clear_stream(previous, &mut affected);
        }
        if let Some(current) = current.filter(|output| !output.text.is_empty()) {
            self.update_stream(current, &mut affected);
        }
        for closed_model_run in closed_model_runs {
            self.close_model_run(closed_model_run, &mut affected);
        }
        self.recompute(affected)
    }

    fn close_invocations<'a>(
        &mut self,
        invocation_refs: impl IntoIterator<Item = &'a str>,
    ) -> Vec<LiveOutputChange> {
        let mut affected = BTreeSet::new();
        for invocation_ref in invocation_refs {
            self.remove_invocation(invocation_ref, &mut affected);
        }
        self.recompute(affected)
    }

    fn register_context(
        &mut self,
        invocation_ref: &str,
        conversation_ref: String,
        generation: Option<u64>,
        target_actor_ref: String,
    ) -> BTreeSet<String> {
        let mut affected = BTreeSet::new();
        if let Some(existing) = self.contexts.get(invocation_ref) {
            affected.insert(existing.conversation_ref.clone());
        }
        let previous_generation = self
            .contexts
            .get(invocation_ref)
            .map(|context| context.generation);
        let generation = generation.or(previous_generation).unwrap_or(0);
        if previous_generation.is_some_and(|previous| previous != generation) {
            self.by_invocation.remove(invocation_ref);
        }
        affected.insert(conversation_ref.clone());
        self.contexts.insert(
            invocation_ref.into(),
            InvocationContext {
                conversation_ref,
                generation,
                target_actor_ref: target_actor_ref.clone(),
            },
        );
        if let Some(output) = self.by_invocation.get_mut(invocation_ref) {
            output.output.target_actor_ref = target_actor_ref;
        }
        affected
    }

    fn update_stream(&mut self, output: &AdapterLiveOutput, affected: &mut BTreeSet<String>) {
        let Some(context) = self.contexts.get(&output.invocation_ref) else {
            return;
        };
        affected.insert(context.conversation_ref.clone());
        let item_key = live_output_key(output);
        let projected = ConversationLiveOutput {
            invocation_ref: output.invocation_ref.clone(),
            generation: context.generation,
            target_actor_ref: context.target_actor_ref.clone(),
            phase: output.phase,
            text: output.text.clone(),
        };
        if self
            .by_invocation
            .get(&output.invocation_ref)
            .is_some_and(|current| current.item_key == item_key && current.output == projected)
        {
            return;
        }
        let update_sequence = self.next_update_sequence();
        self.by_invocation.insert(
            output.invocation_ref.clone(),
            InvocationLiveOutput {
                item_key,
                output: projected,
                update_sequence,
            },
        );
    }

    fn complete_commentary(
        &mut self,
        invocation_ref: &str,
        item_key: LiveOutputItemKey,
        text: &str,
        affected: &mut BTreeSet<String>,
    ) {
        let Some(context) = self.contexts.get(invocation_ref) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if !self.completed_items.insert(item_key.clone()) {
            return;
        }
        affected.insert(context.conversation_ref.clone());
        let projected = ConversationLiveOutput {
            invocation_ref: invocation_ref.into(),
            generation: context.generation,
            target_actor_ref: context.target_actor_ref.clone(),
            phase: AssistantPhase::Commentary,
            text: text.into(),
        };
        self.enqueue_required(LiveOutputChange {
            conversation_ref: context.conversation_ref.clone(),
            output: Some(projected),
        });
        let should_clear = self
            .by_invocation
            .get(invocation_ref)
            .is_some_and(|current| current.item_key == item_key);
        if should_clear {
            self.by_invocation.remove(invocation_ref);
        }
    }

    fn clear_stream(&mut self, output: &AdapterLiveOutput, affected: &mut BTreeSet<String>) {
        let invocation_ref = &output.invocation_ref;
        let should_clear = self
            .by_invocation
            .get(invocation_ref)
            .is_some_and(|current| current.item_key == live_output_key(output));
        if !should_clear {
            return;
        }
        if let Some(context) = self.contexts.get(invocation_ref) {
            affected.insert(context.conversation_ref.clone());
        }
        self.by_invocation.remove(invocation_ref);
    }

    fn close_model_run(
        &mut self,
        closed_model_run: &AdapterClosedModelRun,
        affected: &mut BTreeSet<String>,
    ) {
        let should_clear = self
            .by_invocation
            .get(&closed_model_run.invocation_ref)
            .is_some_and(|current| {
                current.item_key.model_run_ref == closed_model_run.model_run_ref
            });
        if !should_clear {
            return;
        }
        if let Some(context) = self.contexts.get(&closed_model_run.invocation_ref) {
            affected.insert(context.conversation_ref.clone());
        }
        self.by_invocation.remove(&closed_model_run.invocation_ref);
    }

    fn remove_invocation(&mut self, invocation_ref: &str, affected: &mut BTreeSet<String>) {
        if let Some(context) = self.contexts.remove(invocation_ref) {
            affected.insert(context.conversation_ref);
        }
        self.by_invocation.remove(invocation_ref);
        self.completed_items
            .retain(|item| item.invocation_ref != invocation_ref);
    }

    fn recompute(&mut self, affected: BTreeSet<String>) -> Vec<LiveOutputChange> {
        affected
            .into_iter()
            .filter_map(|conversation_ref| {
                let next = self
                    .by_invocation
                    .iter()
                    .filter_map(|(invocation_ref, output)| {
                        let context = self.contexts.get(invocation_ref)?;
                        (context.conversation_ref == conversation_ref).then_some(output)
                    })
                    .max_by_key(|output| output.update_sequence)
                    .map(|output| output.output.clone());
                let previous = self
                    .presented_by_conversation
                    .get(&conversation_ref)
                    .cloned();
                if previous == next {
                    return None;
                }
                if let Some(output) = &next {
                    self.presented_by_conversation
                        .insert(conversation_ref.clone(), output.clone());
                } else {
                    self.presented_by_conversation.remove(&conversation_ref);
                }
                Some(LiveOutputChange {
                    conversation_ref,
                    output: next,
                })
            })
            .collect()
    }

    fn next_update_sequence(&mut self) -> u64 {
        self.update_sequence = self.update_sequence.saturating_add(1);
        self.update_sequence
    }

    fn enqueue_required(&mut self, change: LiveOutputChange) {
        self.delivery_sequence = self.delivery_sequence.saturating_add(1);
        self.required_changes.push_back(RequiredLiveOutputChange {
            delivery_sequence: self.delivery_sequence,
            change,
        });
    }
}

pub(super) struct RuntimeLiveOutputPublisher {
    stop: mpsc::Sender<()>,
    worker: JoinHandle<()>,
}

impl RuntimeLiveOutputPublisher {
    pub(super) fn start(state: SharedState, mailbox: AdapterModelOutputMailbox) -> CwrResult<Self> {
        let (stop, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("cwp-runtime-live-output".into())
            .spawn(move || publish_mailbox(state, mailbox, receiver))?;
        Ok(Self { stop, worker })
    }

    pub(super) fn finish(self) {
        let _ = self.stop.send(());
        let _ = self.worker.join();
    }
}

pub(super) fn observe_runtime_event(
    state: &SharedState,
    conversation_ref: &str,
    event: &LocalRuntimeEvent,
) -> CwrResult<()> {
    let _ = lock_state(state)?
        .live_outputs
        .observe_runtime_event(conversation_ref, event);
    Ok(())
}

pub(super) fn event_conversation_ref(
    runtime: &LocalRuntime,
    event: &LocalRuntimeEvent,
    fallback: &str,
) -> String {
    if let LocalRuntimeEvent::Activity { activity, .. } = event {
        if matches!(
            activity.event,
            ConversationActivityEvent::Progress { .. }
                | ConversationActivityEvent::CommentaryProgress { .. }
        ) {
            if let Ok(Some(conversation_ref)) =
                runtime.published_progress_conversation_ref(&activity.activity_ref)
            {
                return conversation_ref;
            }
        }
    }
    let (invocation_ref, _, _) = runtime_event_identity(event);
    runtime
        .conversation_ref_for_invocation(invocation_ref)
        .ok()
        .flatten()
        .unwrap_or_else(|| fallback.into())
}

pub(super) fn close_invocations(state: &SharedState, invocation_refs: &[String]) -> CwrResult<()> {
    let _ = lock_state(state)?
        .live_outputs
        .close_invocations(invocation_refs.iter().map(String::as_str));
    Ok(())
}

pub(super) fn start_live_output_broadcast(state: SharedState) -> CwrResult<()> {
    thread::Builder::new()
        .name("cwp-runtime-live-output-events".into())
        .spawn(move || {
            let mut published = BTreeMap::new();
            loop {
                thread::sleep(LIVE_OUTPUT_FLUSH_INTERVAL);
                if publish_live_output_changes(&state, &mut published).is_err() {
                    return;
                }
            }
        })?;
    Ok(())
}

fn publish_live_output_changes(
    state: &SharedState,
    published: &mut BTreeMap<String, ConversationLiveOutput>,
) -> CwrResult<()> {
    let _publication = lock_broadcast(state)?;
    let (required, current) = lock_state(state)?.live_outputs.broadcast_snapshot();
    for change in broadcast_changes(published, &required, &current) {
        broadcast_in_publication(
            state,
            RuntimeServiceEvent::ConversationLiveOutput {
                conversation_ref: change.conversation_ref,
                output: change.output,
            },
        )?;
    }
    if let Some(last_required) = required.last() {
        lock_state(state)?
            .live_outputs
            .acknowledge_required_changes(last_required.delivery_sequence);
    }
    *published = current;
    Ok(())
}

fn publish_mailbox(
    state: SharedState,
    mailbox: AdapterModelOutputMailbox,
    stop: mpsc::Receiver<()>,
) {
    let mut last_revision = 0;
    let mut last_output = None;
    loop {
        match stop.recv_timeout(LIVE_OUTPUT_FLUSH_INTERVAL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let _ = publish_mailbox_snapshot(&state, &mailbox, &mut last_revision, &mut last_output);
    }
    let _ = publish_mailbox_snapshot(&state, &mailbox, &mut last_revision, &mut last_output);
}

fn publish_mailbox_snapshot(
    state: &SharedState,
    mailbox: &AdapterModelOutputMailbox,
    last_revision: &mut u64,
    last_output: &mut Option<AdapterLiveOutput>,
) -> CwrResult<()> {
    let snapshot = mailbox.snapshot();
    if snapshot.revision == *last_revision {
        return Ok(());
    }
    let runtime = lock_state(state)?.runtime.clone();
    if runtime.record_streaming_output(snapshot.output.as_ref(), &snapshot.closed_model_runs)? {
        state.graphql_changes.send_replace(());
    }
    *last_revision = snapshot.revision;
    let _ = lock_state(state)?.live_outputs.apply_stream_snapshot(
        last_output.as_ref(),
        snapshot.output.as_ref(),
        &snapshot.closed_model_runs,
    );
    *last_output = snapshot.output;
    Ok(())
}

pub(super) fn broadcast_changes(
    published: &BTreeMap<String, ConversationLiveOutput>,
    required: &[RequiredLiveOutputChange],
    current: &BTreeMap<String, ConversationLiveOutput>,
) -> Vec<LiveOutputChange> {
    let mut changes = required
        .iter()
        .map(|required| required.change.clone())
        .collect::<Vec<_>>();
    let mut after_required = published.clone();
    for required in required {
        match &required.change.output {
            Some(output) => {
                after_required.insert(required.change.conversation_ref.clone(), output.clone());
            }
            None => {
                after_required.remove(&required.change.conversation_ref);
            }
        }
    }
    changes.extend(presentation_changes(&after_required, current));
    changes
}

pub(super) fn presentation_changes(
    previous: &BTreeMap<String, ConversationLiveOutput>,
    current: &BTreeMap<String, ConversationLiveOutput>,
) -> Vec<LiveOutputChange> {
    previous
        .keys()
        .chain(current.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|conversation_ref| {
            let previous = previous.get(&conversation_ref);
            let current = current.get(&conversation_ref);
            (previous != current).then(|| LiveOutputChange {
                conversation_ref,
                output: current.cloned(),
            })
        })
        .collect()
}

fn runtime_event_identity(event: &LocalRuntimeEvent) -> (&str, Option<u64>, &str) {
    match event {
        LocalRuntimeEvent::InvocationStarted {
            invocation_ref,
            generation,
            target_actor_ref,
        } => (invocation_ref, Some(*generation), target_actor_ref),
        LocalRuntimeEvent::Activity {
            invocation_ref,
            target_actor_ref,
            ..
        } => (invocation_ref, None, target_actor_ref),
        LocalRuntimeEvent::Terminal {
            target_actor_ref,
            response,
            ..
        } => (&response.invocation_ref, None, target_actor_ref),
    }
}

fn live_output_key(output: &AdapterLiveOutput) -> LiveOutputItemKey {
    LiveOutputItemKey {
        invocation_ref: output.invocation_ref.clone(),
        model_run_ref: output.model_run_ref.clone(),
        provider: output.provider.clone(),
        thread_id: output.thread_id.clone(),
        response_or_turn_id: output.response_or_turn_id.clone(),
        item_id: output.item_id.clone(),
    }
}

#[cfg(test)]
mod tests;
