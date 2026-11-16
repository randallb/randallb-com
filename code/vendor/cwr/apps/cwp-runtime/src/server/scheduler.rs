use super::*;
use cwr::v2026_09::{conversation::ConversationActivityEvent, local::LocalExecutionTask};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(super) struct ExecutionScheduler {
    started: AtomicBool,
    stopped: AtomicBool,
}

impl ExecutionScheduler {
    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}

pub(super) fn start(state: &SharedState) -> CwrResult<()> {
    if state.scheduler.started.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let weak = Arc::downgrade(state);
    let result = thread::Builder::new()
        .name("cwp-runtime-scheduler".into())
        .spawn(move || {
            let mut schedule_poll = std::time::Instant::now();
            while let Some(state) = weak.upgrade() {
                if state.scheduler.stopped.load(Ordering::Acquire) {
                    break;
                }
                let runtime = match lock_state(&state) {
                    Ok(state) => state.runtime.clone(),
                    Err(_) => break,
                };
                if schedule_poll.elapsed() >= Duration::from_millis(100) {
                    schedule_poll = std::time::Instant::now();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    match runtime.tick_schedules_with_updates(now) {
                        Ok(result) => {
                            let changed = !result.admissions.is_empty()
                                || !result.archived_conversation_refs.is_empty();
                            if changed {
                                state.graphql_changes.send_replace(());
                            }
                            for admission in result.admissions {
                                let _ = broadcast_conversation(
                                    &state,
                                    &runtime,
                                    admission.conversation_ref(),
                                );
                            }
                            for conversation_ref in result.archived_conversation_refs {
                                let _ = broadcast_conversation(&state, &runtime, &conversation_ref);
                            }
                            // Timer admissions and archive controls retain full
                            // publication until their changed roots are scoped.
                            if changed {
                                let _ = broadcast_turn_statuses(&state, &runtime);
                            }
                        }
                        Err(error) => eprintln!("installed schedule worker: {error}"),
                    }
                }
                if let Ok(settled) = runtime.take_settled_stop_invocations() {
                    if !settled.is_empty() {
                        let _ = control::publish_stop_updates(&state, &runtime, &settled, None);
                        state.graphql_changes.send_replace(());
                    }
                }
                match runtime.try_schedule_execution() {
                    Ok(Some(task)) => {
                        // The task owns a reservation before either worker or
                        // mailbox publisher exists. Its drop releases capacity.
                        let result = thread::Builder::new()
                            .name("cwp-runtime-execution".into())
                            .spawn(move || execute(state, runtime, task));
                        if result.is_ok() {
                            continue;
                        }
                        eprintln!(
                            "cwp-runtime could not start execution: {}",
                            result.unwrap_err()
                        );
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("cwp-runtime could not schedule work: {error}"),
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
    if let Err(error) = result {
        state.scheduler.started.store(false, Ordering::Release);
        return Err(error.into());
    }
    Ok(())
}

pub(super) fn execute(state: SharedState, runtime: LocalRuntime, mut task: LocalExecutionTask) {
    let conversation_ref = task
        .record()
        .conversation_ref
        .clone()
        .expect("scheduled conversation");
    let catalog_before = runtime.catalog().ok();
    let mailbox = AdapterModelOutputMailbox::default();
    let publisher = RuntimeLiveOutputPublisher::start(Arc::clone(&state), mailbox.clone())
        .inspect_err(|error| {
            eprintln!("cwp-runtime could not start live-output projection: {error}");
        })
        .ok();
    let event_state = Arc::clone(&state);
    let event_runtime = runtime.clone();
    let fallback_conversation_ref = conversation_ref.clone();
    let target_invocation_ref = task.record().invocation_ref.clone();
    let mut target_terminal_published = false;
    // A traversal cursor for this execution only, not retained status state.
    // Host admissions/controls are retained between observable adapter events.
    let mut published_activity = BTreeMap::from([(target_invocation_ref.clone(), None)]);
    let outcome = task.execute_with_model_output(&mailbox, |event| {
        let invocation_ref = match &event {
            LocalRuntimeEvent::InvocationStarted { invocation_ref, .. }
            | LocalRuntimeEvent::Activity { invocation_ref, .. } => invocation_ref,
            LocalRuntimeEvent::Terminal { response, .. } => &response.invocation_ref,
        };
        published_activity
            .entry(invocation_ref.clone())
            .or_default();
        let event_conversation_ref =
            event_conversation_ref(&event_runtime, &event, &fallback_conversation_ref);
        let _ = observe_live_output(&event_state, &event_conversation_ref, &event);
        if publishes_conversation(&event)
            && broadcast_conversation(&event_state, &event_runtime, &event_conversation_ref)
                .unwrap_or(false)
        {
            let _ = attachments::request_deliveries(&event_state, &event_conversation_ref);
            let published = status::broadcast_execution_turn_statuses(
                &event_state,
                &event_runtime,
                [invocation_ref.as_str()],
                &event_conversation_ref,
                &published_activity,
            );
            if let Ok(cursors) = published {
                published_activity.extend(cursors);
            }
        }
        if matches!(
            &event,
            LocalRuntimeEvent::Terminal { response, .. }
                if response.invocation_ref == target_invocation_ref
        ) {
            target_terminal_published = true;
        }
        let _ = broadcast_execution_event(&event_state, &event_conversation_ref, &event);
    });
    if let (Some(before), Ok(after)) = (catalog_before, runtime.catalog()) {
        if before != after {
            if let Ok(_publication) = lock_broadcast(&state) {
                let _ = broadcast_in_publication(
                    &state,
                    RuntimeServiceEvent::CatalogUpdated { catalog: after },
                );
            }
        }
    }
    if let Some(publisher) = publisher {
        publisher.finish();
    }
    if broadcast_conversation(&state, &runtime, &conversation_ref).unwrap_or(false) {
        let _ = attachments::request_deliveries(&state, &conversation_ref);
        let _ = status::broadcast_execution_turn_statuses(
            &state,
            &runtime,
            published_activity.keys().map(String::as_str),
            &conversation_ref,
            &published_activity,
        );
    }
    if !target_terminal_published {
        if outcome.is_ok() {
            if let Ok(Some((conversation, actor, response))) =
                runtime.terminal_execution_for_invocation(&target_invocation_ref)
            {
                let _ = super::broadcast_execution_terminal(
                    &state,
                    &conversation,
                    &actor,
                    Box::new(response),
                );
            }
        }
        if let Err(error) = &outcome {
            let _ = broadcast_execution_failure(
                &state,
                &conversation_ref,
                &target_invocation_ref,
                &task.record().target_actor_ref,
                &error.to_string(),
            );
        }
    }
    // Settlement can change only invocation records, without a new chat message.
    state.graphql_changes.send_replace(());
    if outcome.is_err() {
        // A storage error can leave the request runnable. Retain the reservation
        // briefly so persistent failures do not create a tight worker retry loop.
        thread::sleep(Duration::from_secs(1));
    }
}

fn publishes_conversation(event: &LocalRuntimeEvent) -> bool {
    // The execution layer has already retained this activity before calling us,
    // and still acknowledges it synchronously. Checkpoints change only recovery
    // state, so publishing the unchanged host projection amplifies large history
    // restorations. Other provider events can contain visible tool activity.
    !matches!(event, LocalRuntimeEvent::Activity { activity, .. }
        if matches!(&activity.event, ConversationActivityEvent::ProviderEvent { method, .. }
            if method == "cwr.adapter_checkpoint"))
}
