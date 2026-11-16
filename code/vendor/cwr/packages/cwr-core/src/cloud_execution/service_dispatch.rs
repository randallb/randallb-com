impl CloudHttpService {
    fn dispatch(&self, dispatch: CloudDispatch) -> CwrResult<CloudAssignment> {
        if dispatch.workspace_ref != self.config.workspace_ref {
            return Err("Wrong cloud workspace".into());
        }
        let normal = self.runtime.validate_cloud_dispatch(&dispatch)?;
        let id = normal
            .invocation_ref
            .rsplit('/')
            .next()
            .ok_or("Invalid CWR invocation identity")?
            .to_owned();
        if self.invocation_ref(&id)? != normal.invocation_ref {
            return Err("Invocation does not belong to this CWR runtime".into());
        }
        let (assignment, already_started) = {
            let mut coordinator = self.lock()?;
            if coordinator
                .existing(&self.context(), &normal.invocation_ref)?
                .is_none()
            {
                coordinator.admit(
                    self.context(),
                    &normal,
                    &dispatch.invocation,
                    super::coordinator::internal_request_id("admit", &normal.invocation_ref),
                    now(),
                )?;
            }
            let record = coordinator.record(&self.context(), &normal.invocation_ref)?;
            if record.terminal.is_some() {
                return Err("Retained invocation is terminal; never dispatch it again".into());
            }
            if record.assignment.is_none() {
                let executor = &self.config.executors[self
                    .placement
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    % self.config.executors.len()];
                coordinator.assign(
                    self.context(),
                    normal.invocation_ref.clone(),
                    record.revision,
                    Assignment {
                        executor_ref: executor.executor_ref.clone(),
                        assignment_ref: format!("{}/assignments/{id}", self.runtime_ref),
                        generation: normal.attempt_count.max(1),
                        route_fingerprint: dispatch.route_fingerprint.clone(),
                        expires_at_unix_ms: now()
                            .checked_add(self.config.assignment_lifetime_ms)
                            .ok_or("Assignment expiry overflow")?,
                    },
                    super::coordinator::internal_request_id("assign", &normal.invocation_ref),
                    now(),
                )?;
            }
            let record = coordinator.record(&self.context(), &normal.invocation_ref)?;
            let retained = record.assignment.clone().ok_or("Assignment disappeared")?;
            if retained.route_fingerprint != dispatch.route_fingerprint {
                return Err("Retained route differs from cloud request".into());
            }
            let started = super::coordinator::executor_start_receipt_available(
                &coordinator.lookup_receipt(
                    &self.context(), &normal.invocation_ref,
                    &super::coordinator::internal_request_id("executor-start", &retained.assignment_ref),
                )?,
            )?;
            let token = coordinator.assignment_token(&self.context(), &normal.invocation_ref)?;
            (
                CloudAssignment {
                    assignment_id: id,
                    invocation_ref: normal.invocation_ref.clone(),
                    executor_ref: retained.executor_ref.clone(),
                    generation: retained.generation,
                    expires_at_unix_ms: retained.expires_at_unix_ms,
                    token,
                },
                started,
            )
        };
        // Never hold the journal lock across transport: the Worker checks its
        // current authority/start reservation through this same native service.
        if !already_started {
            if let Err(error) = self.executor(
                &assignment,
                "start",
                Some(json!({"assignment":assignment,"dispatch":dispatch})),
                None,
            ) {
                self.stop(
                    &normal.invocation_ref,
                    false,
                    "Executor start acknowledgement unavailable",
                )?;
                return Err(error);
            }
        }
        Ok(assignment)
    }

    fn events(&self, id: &str, authorization: &str, after: u64) -> CwrResult<CloudEvents> {
        let (reference, _, assignment) = self.credentials(id, authorization, false)?;
        // A timed-out GET may still be persisting frames. It must not start a
        // second ingestion loop for the same assignment. This guard never holds
        // a runtime/journal lock and is released on every return/error.
        let Some(_poll) = self.reserve_event_poll(id)? else {
            return self.retained_event_batch(&reference, after);
        };
        let record = self.lock()?.record(&self.context(), &reference)?;
        if record.terminal.is_some() {
            return self.retained_event_batch(&reference, after);
        }
        self.runtime.cloud_execution_record(&reference, true)?;
        let cursor = record.next_sequence.saturating_sub(1);
        // Drain accepted durable frames before doing more remote work. The
        // proxy can recover its exact cursor after losing a GET response.
        if after < cursor {
            return self.retained_event_batch(&reference, after);
        }
        let value = match self.executor(&assignment, "events", None, Some(cursor)) {
            Ok(value) => value,
            Err(error) => {
                self.stop(&reference, false, "Executor lost during invocation")?;
                return Err(error);
            }
        };
        let batch: CloudEvents = serde_json::from_value(value)?;
        if batch.events.len() > 64 {
            return Err("Executor event batch exceeds limit".into());
        }
        let started = std::time::Instant::now();
        let mut published = 0;
        for event in batch.events.into_iter().take(8) {
            // Bound work between durable commits as well as by count. A single
            // slow storage operation remains governed by its transport deadline;
            // never abandon/replay a mutation just because this slice elapsed.
            if published > 0 && started.elapsed() >= Duration::from_millis(500) {
                break;
            }
            self.runtime.cloud_execution_record(&reference, true)?;
            self.lock()?.publish_recorded(
                self.context(),
                reference.clone(),
                &assignment.token,
                CloudCallback {
                    request_id: format!("{}:frame:{}", assignment.assignment_id, event.sequence),
                    sequence: event.sequence,
                    frame: event.frame,
                },
                now(),
            )?;
            published += 1;
        }
        if published > 0 {
            let retained = self.lock()?.record(&self.context(), &reference)?;
            let _ = self.executor(
                &assignment,
                "ack",
                Some(json!({"through":retained.next_sequence.saturating_sub(1)})),
                None,
            );
            if retained.terminal.is_some() {
                let _ = self.executor(&assignment, "destroy", Some(json!({})), None);
            }
        }
        self.retained_event_batch(&reference, after)
    }

    fn reserve_event_poll(&self, id: &str) -> CwrResult<Option<EventPoll<'_>>> {
        EventPoll::reserve(&self.event_polls, id)
    }

    fn retained_event_batch(&self, reference: &str, after: u64) -> CwrResult<CloudEvents> {
        let mut coordinator = self.lock()?;
        let record = coordinator.record(&self.context(), reference)?;
        if matches!(
            record.terminal,
            Some(EventKind::Interrupted | EventKind::Cancelled)
        ) {
            return Err("Invocation interrupted or cancelled".into());
        }
        let page = coordinator.retained_events(&self.context(), reference, Some(after), 64)?;
        // Cancellation cannot interleave the journal record/page pair. Fresh
        // role/source authority also fences terminal data after a slow read.
        self.runtime.cloud_execution_delivery_record(reference)?;
        if record.terminal.is_none() {
            self.runtime.cloud_execution_record(reference, true)?;
        }
        let events = page
            .items
            .into_iter()
            .filter(|event| event.payload.get("type").is_some())
            .map(|event| super::CloudEvent {
                sequence: event.sequence,
                frame: event.payload,
            })
            .collect();
        Ok(CloudEvents {
            events,
            terminal: record.terminal.is_some() && page.complete,
        })
    }
}

struct EventPoll<'a> {
    active: &'a Mutex<std::collections::BTreeSet<String>>,
    id: String,
}
impl<'a> EventPoll<'a> {
    fn reserve(
        active: &'a Mutex<std::collections::BTreeSet<String>>,
        id: &str,
    ) -> CwrResult<Option<Self>> {
        let mut entries = active.lock().map_err(|_| "Cloud poll lock poisoned")?;
        if !entries.insert(id.into()) {
            return Ok(None);
        }
        Ok(Some(Self {
            active,
            id: id.into(),
        }))
    }
}
impl Drop for EventPoll<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.id);
        }
    }
}

#[cfg(test)]
#[path = "service_dispatch/poll_tests.rs"]
mod poll_tests;
